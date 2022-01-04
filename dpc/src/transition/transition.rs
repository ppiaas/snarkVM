// Copyright (C) 2019-2021 Aleo Systems Inc.
// This file is part of the snarkVM library.

// The snarkVM library is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// The snarkVM library is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with the snarkVM library. If not, see <https://www.gnu.org/licenses/>.

use crate::{circuits::*, prelude::*};
use snarkvm_algorithms::{
    merkle_tree::{MerklePath, MerkleTree},
    traits::SNARK,
};
use snarkvm_utilities::{FromBytes, FromBytesDeserializer, ToBytes, ToBytesSerializer};

use anyhow::{anyhow, Result};
use serde::{de, ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fmt,
    hash::{Hash, Hasher},
    io::{Read, Result as IoResult, Write},
    str::FromStr,
    sync::Arc,
};

#[derive(Derivative)]
#[derivative(
    Clone(bound = "N: Network"),
    Debug(bound = "N: Network"),
    PartialEq(bound = "N: Network"),
    Eq(bound = "N: Network")
)]
pub struct Transition<N: Network> {
    /// The ID of this transition.
    transition_id: N::TransitionID,
    /// The serial numbers of the input records.
    serial_numbers: Vec<N::SerialNumber>,
    /// The commitments of the output records.
    commitments: Vec<N::Commitment>,
    /// The ciphertexts of the output records.
    ciphertexts: Vec<N::RecordCiphertext>,
    /// A value balance is the difference between the input and output record values.
    value_balance: AleoAmount,
    /// The events emitted from this transition.
    events: Vec<Event<N>>,
    /// The zero-knowledge proof attesting to the validity of this transition.
    proof: N::OuterProof,
}

impl<N: Network> Transition<N> {
    /// Initializes a new instance of a transition.
    #[inline]
    pub(crate) fn new(request: &Request<N>, response: &Response<N>, proof: N::OuterProof) -> Result<Self> {
        // Fetch the serial numbers.
        let serial_numbers = request.to_serial_numbers()?;

        // Fetch the ciphertexts and value balance.
        let transition_id = response.transition_id();
        let ciphertexts = response.ciphertexts();
        let value_balance = response.value_balance();
        let events = response.events().clone();

        // Construct the transition.
        Self::from(transition_id, serial_numbers, ciphertexts, value_balance, events, proof)
    }

    /// Constructs an instance of a transition from the given inputs.
    pub(crate) fn from(
        transition_id: N::TransitionID,
        serial_numbers: Vec<N::SerialNumber>,
        ciphertexts: Vec<N::RecordCiphertext>,
        value_balance: AleoAmount,
        events: Vec<Event<N>>,
        proof: N::OuterProof,
    ) -> Result<Self> {
        // Compute the commitments.
        let commitments = ciphertexts.iter().map(|c| c.commitment()).collect::<Vec<_>>();
        // Construct the transition.
        let transition = Self {
            transition_id: Self::compute_transition_id(&serial_numbers, &commitments)?,
            serial_numbers,
            commitments,
            ciphertexts,
            value_balance,
            events,
            proof,
        };
        // Ensure the transition ID matches.
        match transition_id == transition.transition_id() {
            true => Ok(transition),
            false => Err(anyhow!(
                "Incorrect transition ID during deserialization. Expected {}, found {}",
                transition_id,
                transition.transition_id(),
            )),
        }
    }

    /// Returns `true` if the transition ID is well-formed and the transition proof is valid.
    #[inline]
    pub fn verify(
        &self,
        inner_circuit_id: N::InnerCircuitID,
        ledger_root: N::LedgerRoot,
        local_transitions_root: N::TransactionID,
    ) -> bool {
        // Ensure the number of events is less than `N::NUM_EVENTS`.
        if self.events.len() > N::NUM_EVENTS as usize {
            eprintln!("Transition contains an invalid number of events");
            return false;
        }

        // Returns `false` if the transition proof is invalid.
        match N::OuterSNARK::verify(
            N::outer_verifying_key(),
            &OuterPublicVariables::new(
                InnerPublicVariables::new(
                    self.transition_id,
                    self.value_balance,
                    ledger_root,
                    local_transitions_root,
                    None,
                ),
                &inner_circuit_id,
            ),
            &self.proof,
        ) {
            Ok(is_valid) => match is_valid {
                true => true,
                false => {
                    eprintln!("Transition proof failed to verify");
                    false
                }
            },
            Err(error) => {
                eprintln!("Failed to validate transition proof: {:?}", error);
                false
            }
        }
    }

    /// Returns `true` if the given serial number exists.
    pub fn contains_serial_number(&self, serial_number: &N::SerialNumber) -> bool {
        self.serial_numbers.contains(serial_number)
    }

    /// Returns `true` if the given commitment exists.
    pub fn contains_commitment(&self, commitment: &N::Commitment) -> bool {
        self.commitments.contains(commitment)
    }

    /// Returns the transition ID.
    #[inline]
    pub fn transition_id(&self) -> N::TransitionID {
        self.transition_id
    }

    /// Returns a reference to the serial numbers.
    #[inline]
    pub fn serial_numbers(&self) -> impl Iterator<Item = &N::SerialNumber> + fmt::Debug + '_ {
        self.serial_numbers.iter()
    }

    /// Returns a reference to the commitments.
    #[inline]
    pub fn commitments(&self) -> impl Iterator<Item = &N::Commitment> + fmt::Debug + '_ {
        self.commitments.iter()
    }

    /// Returns a reference to the ciphertexts.
    #[inline]
    pub fn ciphertexts(&self) -> impl Iterator<Item = &N::RecordCiphertext> + fmt::Debug + '_ {
        self.ciphertexts.iter()
    }

    /// Returns a reference to the value balance.
    #[inline]
    pub fn value_balance(&self) -> &AleoAmount {
        &self.value_balance
    }

    /// Returns a reference to the events.
    #[inline]
    pub fn events(&self) -> impl Iterator<Item = &Event<N>> + fmt::Debug + '_ {
        self.events.iter()
    }

    /// Returns a reference to the transition proof.
    #[inline]
    pub fn proof(&self) -> &N::OuterProof {
        &self.proof
    }

    /// Returns the decrypted records using record view key events, if they exist.
    #[inline]
    pub fn to_records(&self) -> impl Iterator<Item = Record<N>> + fmt::Debug + '_ {
        let ciphertexts: Vec<&N::RecordCiphertext> = self.ciphertexts().collect();
        self.events
            .iter()
            .filter_map(move |event| match event {
                Event::RecordViewKey(i, record_view_key) => match ciphertexts.get(*i as usize) {
                    Some(ciphertext) => Record::from_record_view_key(record_view_key.clone(), *ciphertext).ok(),
                    None => None,
                },
                _ => None,
            })
            .filter(|record| !record.is_dummy())
    }

    /// Returns an inclusion proof for the transition tree.
    #[inline]
    pub fn to_transition_inclusion_proof(&self, leaf: impl ToBytes) -> Result<MerklePath<N::TransitionIDParameters>> {
        // Convert the leaf into bytes.
        let leaf = leaf.to_bytes_le()?;

        // Retrieve the transition leaves.
        let leaves = Self::compute_transition_leaves(&self.serial_numbers, &self.commitments)?;

        // Find the index of the given leaf.
        for (index, candidate_leaf) in leaves.iter().enumerate() {
            if *candidate_leaf == leaf {
                let tree = MerkleTree::<N::TransitionIDParameters>::new(
                    Arc::new(N::transition_id_parameters().clone()),
                    &leaves,
                )?;
                return Ok(tree.generate_proof(index, &leaf)?);
            }
        }

        Err(anyhow!("Failed to find the given element in the transition"))
    }

    ///
    /// Returns the transition ID, which is the root of transition tree.
    ///
    #[inline]
    pub(crate) fn compute_transition_id(
        serial_numbers: &[N::SerialNumber],
        commitments: &[N::Commitment],
    ) -> Result<N::TransitionID> {
        let leaves = Self::compute_transition_leaves(serial_numbers, commitments)?;
        let tree =
            MerkleTree::<N::TransitionIDParameters>::new(Arc::new(N::transition_id_parameters().clone()), &leaves)?;
        Ok((*tree.root()).into())
    }

    ///
    /// Returns an instance of the transition tree.
    ///
    /// Transition Tree := MerkleTree(serial numbers || commitments)
    ///
    #[inline]
    pub(crate) fn compute_transition_leaves(
        serial_numbers: &[N::SerialNumber],
        commitments: &[N::Commitment],
    ) -> Result<Vec<Vec<u8>>> {
        // Construct the leaves of the transition tree.
        let leaves: Vec<Vec<u8>> = vec![
            // Leaf 0, 1 := serial numbers
            serial_numbers
                .iter()
                .take(N::NUM_INPUT_RECORDS)
                .map(ToBytes::to_bytes_le)
                .collect::<Result<Vec<_>>>()?,
            // Leaf 2, 3 := commitments
            commitments
                .iter()
                .take(N::NUM_OUTPUT_RECORDS)
                .map(ToBytes::to_bytes_le)
                .collect::<Result<Vec<_>>>()?,
        ]
        .concat();

        // Ensure the correct number of leaves are allocated.
        assert_eq!(usize::pow(2, N::TRANSITION_TREE_DEPTH as u32), leaves.len());

        Ok(leaves)
    }
}

impl<N: Network> FromBytes for Transition<N> {
    #[inline]
    fn read_le<R: Read>(mut reader: R) -> IoResult<Self> {
        let transition_id: N::TransitionID = FromBytes::read_le(&mut reader)?;

        let mut serial_numbers = Vec::<N::SerialNumber>::with_capacity(N::NUM_INPUT_RECORDS);
        for _ in 0..N::NUM_INPUT_RECORDS {
            serial_numbers.push(FromBytes::read_le(&mut reader)?);
        }

        let mut ciphertexts = Vec::<N::RecordCiphertext>::with_capacity(N::NUM_OUTPUT_RECORDS);
        for _ in 0..N::NUM_OUTPUT_RECORDS {
            ciphertexts.push(FromBytes::read_le(&mut reader)?);
        }

        let value_balance: AleoAmount = FromBytes::read_le(&mut reader)?;

        let num_events: u16 = FromBytes::read_le(&mut reader)?;
        let mut events = Vec::with_capacity(num_events as usize);
        for _ in 0..num_events {
            events.push(FromBytes::read_le(&mut reader)?);
        }

        let proof: N::OuterProof = FromBytes::read_le(&mut reader)?;

        Ok(
            Self::from(transition_id, serial_numbers, ciphertexts, value_balance, events, proof)
                .expect("Failed to deserialize a transition from bytes"),
        )
    }
}

impl<N: Network> ToBytes for Transition<N> {
    #[inline]
    fn write_le<W: Write>(&self, mut writer: W) -> IoResult<()> {
        self.transition_id.write_le(&mut writer)?;
        self.serial_numbers.write_le(&mut writer)?;
        self.ciphertexts.write_le(&mut writer)?;
        self.value_balance.write_le(&mut writer)?;
        (self.events.len() as u16).write_le(&mut writer)?;
        self.events.write_le(&mut writer)?;
        self.proof.write_le(&mut writer)
    }
}

impl<N: Network> FromStr for Transition<N> {
    type Err = anyhow::Error;

    fn from_str(transition: &str) -> Result<Self, Self::Err> {
        Ok(serde_json::from_str(transition)?)
    }
}

impl<N: Network> fmt::Display for Transition<N> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(self).map_err::<fmt::Error, _>(serde::ser::Error::custom)?
        )
    }
}

impl<N: Network> Serialize for Transition<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match serializer.is_human_readable() {
            true => {
                let mut transition = serializer.serialize_struct("Transition", 7)?;
                transition.serialize_field("transition_id", &self.transition_id)?;
                transition.serialize_field("serial_numbers", &self.serial_numbers)?;
                transition.serialize_field("commitments", &self.commitments)?;
                transition.serialize_field("ciphertexts", &self.ciphertexts)?;
                transition.serialize_field("value_balance", &self.value_balance)?;
                transition.serialize_field("events", &self.events)?;
                transition.serialize_field("proof", &self.proof)?;
                transition.end()
            }
            false => ToBytesSerializer::serialize_with_size_encoding(self, serializer),
        }
    }
}

impl<'de, N: Network> Deserialize<'de> for Transition<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match deserializer.is_human_readable() {
            true => {
                let transition = serde_json::Value::deserialize(deserializer)?;
                // Recover the transition.
                Self::from(
                    serde_json::from_value(transition["transition_id"].clone()).map_err(de::Error::custom)?,
                    serde_json::from_value(transition["serial_numbers"].clone()).map_err(de::Error::custom)?,
                    serde_json::from_value(transition["ciphertexts"].clone()).map_err(de::Error::custom)?,
                    serde_json::from_value(transition["value_balance"].clone()).map_err(de::Error::custom)?,
                    serde_json::from_value(transition["events"].clone()).map_err(de::Error::custom)?,
                    serde_json::from_value(transition["proof"].clone()).map_err(de::Error::custom)?,
                )
                .map_err(de::Error::custom)
            }
            false => FromBytesDeserializer::<Self>::deserialize_with_size_encoding(deserializer, "transition"),
        }
    }
}

impl<N: Network> Hash for Transition<N> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.transition_id.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testnet2::Testnet2;

    #[test]
    fn test_size() {
        {
            use crate::testnet1::Testnet1;
            let transaction = Testnet1::genesis_block().to_coinbase_transaction().unwrap();
            let transition = transaction.transitions().first().unwrap().clone();
            let transition_bytes = transition.to_bytes_le().unwrap();
            assert_eq!(1039, transition_bytes.len(),);
        }
        {
            let transaction = Testnet2::genesis_block().to_coinbase_transaction().unwrap();
            let transition = transaction.transitions().first().unwrap().clone();
            let transition_bytes = transition.to_bytes_le().unwrap();
            assert_eq!(1039, transition_bytes.len(),);
        }
    }

    #[test]
    fn test_transition_serde_json() {
        let transaction = Testnet2::genesis_block().to_coinbase_transaction().unwrap();
        let expected_transition = transaction.transitions().first().unwrap().clone();

        // Serialize
        let expected_string = expected_transition.to_string();
        let candidate_string = serde_json::to_string(&expected_transition).unwrap();
        assert_eq!(2074, candidate_string.len(), "Update me if serialization has changed");
        assert_eq!(expected_string, candidate_string);

        // Deserialize
        assert_eq!(expected_transition, Transition::from_str(&candidate_string).unwrap());
        assert_eq!(expected_transition, serde_json::from_str(&candidate_string).unwrap());
    }

    #[test]
    fn test_transition_bincode() {
        let transaction = Testnet2::genesis_block().to_coinbase_transaction().unwrap();
        let expected_transition = transaction.transitions().first().unwrap().clone();

        // Serialize
        let expected_bytes = expected_transition.to_bytes_le().unwrap();
        let candidate_bytes = bincode::serialize(&expected_transition).unwrap();
        assert_eq!(1039, expected_bytes.len(), "Update me if serialization has changed");
        // TODO (howardwu): Serialization - Handle the inconsistency between ToBytes and Serialize (off by a length encoding).
        assert_eq!(&expected_bytes[..], &candidate_bytes[8..]);

        // Deserialize
        assert_eq!(expected_transition, Transition::read_le(&expected_bytes[..]).unwrap());
        assert_eq!(expected_transition, bincode::deserialize(&candidate_bytes[..]).unwrap());
    }
}
