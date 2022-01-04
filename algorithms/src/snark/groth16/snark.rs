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

use super::{
    create_random_proof,
    generate_random_parameters,
    verify_proof,
    PreparedVerifyingKey,
    Proof,
    ProvingKey,
    VerifyingKey,
};
use crate::{SNARKError, SNARK, SRS};
use snarkvm_curves::traits::PairingEngine;
use snarkvm_fields::ToConstraintField;
use snarkvm_r1cs::ConstraintSynthesizer;

use rand::{CryptoRng, Rng};
use std::{marker::PhantomData, sync::atomic::AtomicBool};

/// Note: V should serialize its contents to `Vec<E::Fr>` in the same order as
/// during the constraint generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Groth16<E: PairingEngine, V: ToConstraintField<E::Fr> + Clone> {
    _phantom: PhantomData<(E, V)>,
}

impl<E: PairingEngine, V: ToConstraintField<E::Fr> + Clone> SNARK for Groth16<E, V> {
    type BaseField = E::Fq;
    type PreparedVerifyingKey = PreparedVerifyingKey<E>;
    type Proof = Proof<E>;
    type ProvingKey = ProvingKey<E>;
    type ScalarField = E::Fr;
    type UniversalSetupConfig = usize;
    type UniversalSetupParameters = ();
    type VerifierInput = V;
    type VerifyingKey = VerifyingKey<E>;

    fn setup<C: ConstraintSynthesizer<E::Fr>, R: Rng + CryptoRng>(
        circuit: &C,
        srs: &mut SRS<R, Self::UniversalSetupParameters>,
    ) -> Result<(Self::ProvingKey, Self::VerifyingKey), SNARKError> {
        let setup_time = start_timer!(|| "{Groth 2016}::Setup");
        let pp = match srs {
            SRS::CircuitSpecific(rng) => generate_random_parameters::<E, C, _>(circuit, *rng)?,
            _ => return Err(SNARKError::ExpectedCircuitSpecificSRS),
        };
        let vk = pp.vk.clone();
        end_timer!(setup_time);
        Ok((pp, vk))
    }

    // terminator not implemented for Groth16
    fn prove_with_terminator<C: ConstraintSynthesizer<E::Fr>, R: Rng + CryptoRng>(
        proving_key: &Self::ProvingKey,
        input_and_witness: &C,
        _terminator: &AtomicBool,
        rng: &mut R,
    ) -> Result<Self::Proof, SNARKError> {
        let proof_time = start_timer!(|| "{Groth 2016}::Prove");
        let result = create_random_proof::<E, C, _>(input_and_witness, proving_key, rng)?;
        end_timer!(proof_time);
        Ok(result)
    }

    fn verify_prepared(
        prepared_verifying_key: &Self::PreparedVerifyingKey,
        input: &Self::VerifierInput,
        proof: &Self::Proof,
    ) -> Result<bool, SNARKError> {
        let verify_time = start_timer!(|| "{Groth 2016}::Verify");
        let conversion_time = start_timer!(|| "Convert input to E::Fr");
        let input = input.to_field_elements()?;
        end_timer!(conversion_time);
        let verification = start_timer!(|| format!("Verify proof w/ input len: {}", input.len()));
        let result = verify_proof(prepared_verifying_key, proof, &input)?;
        end_timer!(verification);
        end_timer!(verify_time);
        Ok(result)
    }
}
