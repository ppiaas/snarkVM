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

use core::marker::PhantomData;

use crate::{
    constraints::{
        ahp::AHPForR1CS,
        proof::ProofVar,
        snark::MarlinSNARK,
        verifier_key::{CircuitVerifyingKeyVar, PreparedCircuitVerifyingKeyVar},
    },
    marlin::{MarlinError, MarlinMode},
    FiatShamirAlgebraicSpongeRng,
    FiatShamirAlgebraicSpongeRngVar,
    FiatShamirRng,
    FiatShamirRngVar,
    PolynomialCommitment,
    PoseidonSponge,
    PoseidonSpongeGadget,
    String,
    Vec,
};
use snarkvm_algorithms::{crypto_hash::PoseidonDefaultParametersField, fft::EvaluationDomain};
use snarkvm_fields::{FieldParameters, PrimeField};
use snarkvm_gadgets::{
    bits::Boolean,
    nonnative::{params::OptimizationType, NonNativeFieldInputVar, NonNativeFieldVar},
    traits::{algorithms::SNARKVerifierGadget, eq::EqGadget, fields::FieldGadget},
    AllocGadget,
    Integer,
    PrepareGadget,
    ToBytesGadget,
    UInt8,
};
use snarkvm_polycommit::{PCCheckRandomDataVar, PCCheckVar};
use snarkvm_r1cs::{ConstraintSystem, SynthesisError, ToConstraintField};

/// The Marlin verification gadget.
pub struct MarlinVerificationGadget<
    TargetField: PrimeField,
    BaseField: PrimeField,
    PC: PolynomialCommitment<TargetField, BaseField>,
    PCG: PCCheckVar<TargetField, PC, BaseField>,
    MM: MarlinMode,
>(PhantomData<(TargetField, BaseField, PC, PCG, MM)>);

/// Fiat Shamir Algebraic Sponge RNG type
pub type FSA<InnerField, OuterField> =
    FiatShamirAlgebraicSpongeRng<InnerField, OuterField, PoseidonSponge<OuterField, 6, 1>>;

/// Fiat Shamir Algebraic Sponge RNG Gadget type
pub type FSG<InnerField, OuterField> = FiatShamirAlgebraicSpongeRngVar<
    InnerField,
    OuterField,
    PoseidonSponge<OuterField, 6, 1>,
    PoseidonSpongeGadget<OuterField, 6, 1>,
>;

impl<TargetField, BaseField, PC, PCG, FS, MM, V> SNARKVerifierGadget<MarlinSNARK<TargetField, BaseField, PC, FS, MM, V>>
    for MarlinVerificationGadget<TargetField, BaseField, PC, PCG, MM>
where
    TargetField: PrimeField,
    BaseField: PrimeField + PoseidonDefaultParametersField,
    PC: PolynomialCommitment<TargetField, BaseField>,
    PCG: PCCheckVar<TargetField, PC, BaseField>,
    FS: FiatShamirRng<TargetField, BaseField>,
    MM: MarlinMode,
    V: ToConstraintField<TargetField> + Clone,
{
    type InputGadget = NonNativeFieldInputVar<TargetField, BaseField>;
    type PreparedVerificationKeyGadget = PreparedCircuitVerifyingKeyVar<
        TargetField,
        BaseField,
        PC,
        PCG,
        FSA<TargetField, BaseField>,
        FSG<TargetField, BaseField>,
        MM,
    >;
    type ProofGadget = ProofVar<TargetField, BaseField, PC, PCG>;
    type VerificationKeyGadget = CircuitVerifyingKeyVar<TargetField, BaseField, PC, PCG, MM>;

    fn input_gadget_from_bytes<CS: ConstraintSystem<BaseField>>(
        mut cs: CS,
        bytes: &[UInt8],
    ) -> Result<Self::InputGadget, SynthesisError> {
        // First, we allocate the input according to the `ToConstraintField` impl wrt S::BaseField.
        let max_size = (TargetField::Parameters::CAPACITY / 8) as usize;
        // Obtain the values of the field elements corresponding to each `max_size` chunk.
        let values = bytes.iter().map(|byte| byte.value.unwrap_or(0u8)).collect::<Vec<_>>();
        let values: Vec<TargetField> = values.to_field_elements().unwrap();

        // Allocate the input as non-native field elements.
        let input = NonNativeFieldInputVar::alloc_checked(cs.ns(|| "Construct input var"), || Ok(values))?;
        // Convert the input to bytes
        let input_bytes = input.val.to_bytes_strict(cs.ns(|| "Input to bytes"))?;

        // Chunk and pad the original input bytes according to the `ToConstraintField` impl
        let bytes_per_fe = ((TargetField::Parameters::MODULUS_BITS + 7) / 8) as usize;
        let padded_bytes = bytes
            .chunks(max_size)
            .flat_map(|chunk| {
                let mut chunk = chunk.to_vec();
                chunk.resize(bytes_per_fe, UInt8::constant(0));
                chunk
            })
            .collect::<Vec<_>>();
        padded_bytes.enforce_equal(cs.ns(|| "Enforce equality between bytes"), &input_bytes)?;
        Ok(input)
    }

    fn check_verify<CS: ConstraintSystem<BaseField>>(
        mut cs: CS,
        verification_key: &Self::VerificationKeyGadget,
        input: &Self::InputGadget,
        proof: &Self::ProofGadget,
    ) -> Result<(), SynthesisError> {
        let pvk = verification_key.prepare(cs.ns(|| "prepare vk"))?;
        <Self as SNARKVerifierGadget<MarlinSNARK<TargetField, BaseField, PC, FS, MM, V>>>::prepared_check_verify(
            cs, &pvk, input, proof,
        )
    }

    fn prepared_check_verify<CS: ConstraintSystem<BaseField>>(
        mut cs: CS,
        prepared_verification_key: &Self::PreparedVerificationKeyGadget,
        input: &Self::InputGadget,
        proof: &Self::ProofGadget,
    ) -> Result<(), SynthesisError> {
        let result = Self::prepared_verify(
            cs.ns(|| "prepared_verify"),
            prepared_verification_key,
            &input.val,
            proof,
        )
        .unwrap();
        result.enforce_equal(cs.ns(|| "enforce_verification_correctness"), &Boolean::Constant(true))?;
        Ok(())
    }
}

impl<TargetField, BaseField, PC, PCG, MM: MarlinMode> MarlinVerificationGadget<TargetField, BaseField, PC, PCG, MM>
where
    TargetField: PrimeField,
    BaseField: PrimeField + PoseidonDefaultParametersField,
    PC: PolynomialCommitment<TargetField, BaseField>,
    PCG: PCCheckVar<TargetField, PC, BaseField>,
{
    /// The encoding of the protocol name for use as seed.
    pub const PROTOCOL_NAME: &'static [u8] = b"MARLIN-2019";

    /// Verify with an established hashchain initial state.
    pub fn prepared_verify<
        CS: ConstraintSystem<BaseField>,
        PR: FiatShamirRng<TargetField, BaseField>,
        R: FiatShamirRngVar<TargetField, BaseField, PR>,
    >(
        mut cs: CS,
        prepared_verifying_key: &PreparedCircuitVerifyingKeyVar<TargetField, BaseField, PC, PCG, PR, R, MM>,
        public_input: &[NonNativeFieldVar<TargetField, BaseField>],
        proof: &ProofVar<TargetField, BaseField, PC, PCG>,
    ) -> Result<Boolean, MarlinError> {
        let mut fs_rng = prepared_verifying_key.fs_rng.clone();

        if cfg!(debug_assertions) {
            eprintln!("before AHP: constraints: {}", cs.num_constraints());
        }

        let padded_public_input = {
            let mut new_input = vec![NonNativeFieldVar::<TargetField, BaseField>::one(&mut cs.ns(|| "one"))?];
            new_input.extend_from_slice(public_input);
            let domain_x = EvaluationDomain::<TargetField>::new(new_input.len()).unwrap();
            new_input.resize(
                core::cmp::max(new_input.len(), domain_x.size()),
                NonNativeFieldVar::<TargetField, BaseField>::Constant(TargetField::zero()),
            );
            new_input
        };

        // let input_bytes = padded_public_input.to_bytes_strict(&mut cs.ns(|| "input_to_bytes"))?;
        fs_rng.absorb_nonnative_field_elements(
            &mut cs.ns(|| "absorb_input_bytes"),
            &padded_public_input,
            OptimizationType::Weight,
        )?;

        let (_, verifier_state) = AHPForR1CS::<TargetField, BaseField, PC, PCG, MM>::verifier_first_round(
            cs.ns(|| "verifier_first_round"),
            prepared_verifying_key.domain_h_size,
            prepared_verifying_key.domain_k_size,
            &mut fs_rng,
            &proof.commitments[0],
            &proof.prover_messages[0].field_elements,
        )?;

        let (_, verifier_state) = AHPForR1CS::<TargetField, BaseField, PC, PCG, MM>::verifier_second_round(
            cs.ns(|| "verifier_second_round"),
            verifier_state,
            &mut fs_rng,
            &proof.commitments[1],
            &proof.prover_messages[1].field_elements,
        )?;

        let verifier_state = AHPForR1CS::<TargetField, BaseField, PC, PCG, MM>::verifier_third_round(
            cs.ns(|| "verifier_third_round"),
            verifier_state,
            &mut fs_rng,
            &proof.commitments[2],
            &proof.prover_messages[2].field_elements,
        )?;

        let lc = AHPForR1CS::<TargetField, BaseField, PC, PCG, MM>::verifier_decision(
            cs.ns(|| "verifier_decision"),
            &padded_public_input,
            &proof.evaluations,
            verifier_state.clone(),
            &prepared_verifying_key.domain_k_size_gadget,
        )?;

        let (num_opening_challenges, num_batching_rands, comm, query_set, evaluations) =
            AHPForR1CS::<TargetField, BaseField, PC, PCG, MM>::verifier_comm_query_eval_set(
                cs.ns(|| "verifier_comm_query_eval_set"),
                prepared_verifying_key,
                proof,
                &verifier_state,
            )?;

        let mut evaluations_labels = Vec::<(String, NonNativeFieldVar<TargetField, BaseField>)>::new();
        for q in query_set.0.iter().cloned() {
            evaluations_labels.push((q.0.clone(), (q.1).value.clone()));
        }
        evaluations_labels.sort_by(|a, b| a.0.cmp(&b.0));

        let mut evals_vec: Vec<NonNativeFieldVar<TargetField, BaseField>> = Vec::new();
        for (label, point) in evaluations_labels.iter() {
            if label != "outer_sumcheck" && label != "inner_sumcheck" {
                evals_vec.push(evaluations.get_lc_eval(label, point).unwrap());
            }
        }

        fs_rng.absorb_nonnative_field_elements(
            cs.ns(|| "final_absorb_nonnative_field_elements"),
            &evals_vec,
            OptimizationType::Weight,
        )?;

        let (opening_challenges, opening_challenges_bits) = fs_rng.squeeze_128_bits_field_elements_and_bits(
            cs.ns(|| "opening_squeeze_128_bits_field_elements_and_bits"),
            num_opening_challenges,
        )?;
        let (batching_rands, batching_rands_bits) = fs_rng.squeeze_128_bits_field_elements_and_bits(
            cs.ns(|| "batching_squeeze_128_bits_field_elements_and_bits"),
            num_batching_rands,
        )?;

        if cfg!(debug_assertions) {
            eprintln!("before PC checks: constraints: {}", cs.num_constraints());
        }

        let rand_data = PCCheckRandomDataVar::<TargetField, BaseField> {
            opening_challenges,
            opening_challenges_bits,
            batching_rands,
            batching_rands_bits,
        };

        Ok(PCG::prepared_check_combinations(
            cs.ns(|| "prepared_check_combinations"),
            &prepared_verifying_key.prepared_verifier_key,
            &lc,
            &comm,
            &query_set,
            &evaluations,
            &proof.pc_batch_proof,
            &rand_data,
        )?)
    }

    /// Verify with an established hashchain initial state.
    pub fn verify<
        CS: ConstraintSystem<BaseField>,
        PR: FiatShamirRng<TargetField, BaseField>,
        R: FiatShamirRngVar<TargetField, BaseField, PR>,
    >(
        mut cs: CS,
        verifying_key: &CircuitVerifyingKeyVar<TargetField, BaseField, PC, PCG, MM>,
        public_input: &[NonNativeFieldVar<TargetField, BaseField>],
        proof: &ProofVar<TargetField, BaseField, PC, PCG>,
    ) -> Result<Boolean, MarlinError> {
        eprintln!("before prepared_VK: constraints: {}", cs.num_constraints());

        let prepared_verifying_key = verifying_key.prepare(cs.ns(|| "prepare"))?;
        Self::prepared_verify::<_, PR, R>(
            cs.ns(|| "prepared_verify"),
            &prepared_verifying_key,
            public_input,
            proof,
        )
    }
}

#[cfg(test)]
#[allow(clippy::upper_case_acronyms)]
mod test {
    use core::ops::MulAssign;

    use hashbrown::HashMap;

    use snarkvm_curves::{
        bls12_377::{Bls12_377, Fq, Fr},
        bw6_761::BW6_761,
    };
    use snarkvm_gadgets::{
        curves::bls12_377::PairingGadget as Bls12_377PairingGadget,
        traits::{alloc::AllocGadget, eq::EqGadget},
    };
    use snarkvm_polycommit::sonic_pc::{
        commitment::commitment::CommitmentVar,
        proof::batch_lc_proof::BatchLCProofVar,
        sonic_kzg10::SonicKZG10Gadget,
        SonicKZG10,
    };
    use snarkvm_r1cs::TestConstraintSystem;
    use snarkvm_utilities::{test_rng, UniformRand};

    use crate::{
        constraints::{proof::ProverMessageVar, snark::test::Circuit},
        fiat_shamir::{
            FiatShamirAlgebraicSpongeRng,
            FiatShamirAlgebraicSpongeRngVar,
            PoseidonSponge,
            PoseidonSpongeGadget as PoseidonSpongeVar,
        },
        marlin::{MarlinRecursiveMode, MarlinSNARK as MarlinCore, Proof},
    };

    use super::*;

    type PC = SonicKZG10<Bls12_377>;
    type PCGadget = SonicKZG10Gadget<Bls12_377, BW6_761, Bls12_377PairingGadget>;

    type FS = FiatShamirAlgebraicSpongeRng<Fr, Fq, PoseidonSponge<Fq, 6, 1>>;
    type FSG = FiatShamirAlgebraicSpongeRngVar<Fr, Fq, PoseidonSponge<Fq, 6, 1>, PoseidonSpongeVar<Fq, 6, 1>>;

    type MarlinInst = MarlinCore<Fr, Fq, PC, FS, MarlinRecursiveMode>;

    #[test]
    fn verifier_test() {
        let rng = &mut test_rng();

        let max_degree = crate::ahp::AHPForR1CS::<Fr, MarlinRecursiveMode>::max_degree(10000, 25, 10000).unwrap();
        let universal_srs = MarlinInst::universal_setup(max_degree, rng).unwrap();

        let num_constraints = 10000;
        let num_variables = 25;

        let a = Fr::rand(rng);
        let b = Fr::rand(rng);
        let mut c = a;
        c.mul_assign(&b);

        let circ = Circuit {
            a: Some(a),
            b: Some(b),
            num_constraints,
            num_variables,
        };

        let (circuit_pk, circuit_vk) = MarlinInst::circuit_setup(&universal_srs, &circ).unwrap();
        println!("Called index");

        let proof = MarlinInst::prove(&circuit_pk, &circ, rng).unwrap();
        println!("Called prover");

        assert!(MarlinInst::verify(&circuit_vk, &[c], &proof).unwrap());
        println!("Called verifier");
        println!("\nShould not verify (i.e. verifier messages should print below):");
        assert!(!MarlinInst::verify(&circuit_vk, &[a], &proof).unwrap());

        // Native works; now convert to the constraint world!

        let mut cs = TestConstraintSystem::<Fq>::new();

        // BEGIN: ivk to ivk_gadget
        let ivk_gadget: CircuitVerifyingKeyVar<Fr, Fq, PC, PCGadget, MarlinRecursiveMode> =
            CircuitVerifyingKeyVar::alloc(cs.ns(|| "alloc_circuit_vk"), || Ok(circuit_vk)).unwrap();
        // END: ivk to ivk_gadget

        // BEGIN: public input to public_input_gadget
        let public_input: Vec<Fr> = vec![c];

        let public_input_gadget: Vec<NonNativeFieldVar<Fr, Fq>> = public_input
            .iter()
            .enumerate()
            .map(|(i, x)| NonNativeFieldVar::alloc_input(cs.ns(|| format!("alloc_input_{}", i)), || Ok(x)).unwrap())
            .collect();
        // END: public input to public_input_gadget

        // BEGIN: proof to proof_gadget
        let Proof {
            commitments,
            evaluations,
            prover_messages,
            pc_proof,
            ..
        } = proof;

        let commitment_gadgets: Vec<Vec<CommitmentVar<Bls12_377, BW6_761, Bls12_377PairingGadget>>> = commitments
            .iter()
            .enumerate()
            .map(|(i, lst)| {
                lst.iter()
                    .enumerate()
                    .map(|(j, comm)| {
                        CommitmentVar::alloc(cs.ns(|| format!("alloc_commitment_{}_{}", i, j)), || Ok(comm)).unwrap()
                    })
                    .collect()
            })
            .collect();

        let evaluation_gadgets_vec: Vec<NonNativeFieldVar<Fr, Fq>> = evaluations
            .iter()
            .enumerate()
            .map(|(i, eval)| {
                NonNativeFieldVar::alloc(cs.ns(|| format!("alloc_evaluation_{}", i)), || Ok(eval)).unwrap()
            })
            .collect();

        let prover_message_gadgets: Vec<ProverMessageVar<Fr, Fq>> = prover_messages
            .iter()
            .enumerate()
            .map(|(i, msg)| {
                let field_elements: Vec<NonNativeFieldVar<Fr, Fq>> = match msg.field_elements.is_empty() {
                    true => Vec::new(),
                    false => msg
                        .field_elements
                        .iter()
                        .map(|elem| {
                            NonNativeFieldVar::alloc(cs.ns(|| format!("alloc_prover message_{}", i)), || Ok(elem))
                                .unwrap()
                        })
                        .collect(),
                };

                ProverMessageVar { field_elements }
            })
            .collect();

        let pc_batch_proof =
            BatchLCProofVar::<Bls12_377, BW6_761, Bls12_377PairingGadget>::alloc(cs.ns(|| "alloc#proof"), || {
                Ok(pc_proof)
            })
            .unwrap();

        let mut evaluation_gadgets = HashMap::<String, NonNativeFieldVar<Fr, Fq>>::new();

        const ALL_POLYNOMIALS: [&str; 7] = [
            "g_1",
            "g_2",
            "t",
            "vanishing_poly_h_alpha",
            "vanishing_poly_h_beta",
            "vanishing_poly_k_gamma",
            "z_b",
        ];

        for (s, eval) in ALL_POLYNOMIALS.iter().zip(evaluation_gadgets_vec.iter()) {
            evaluation_gadgets.insert(s.to_string(), (*eval).clone());
        }

        let proof_gadget: ProofVar<Fr, Fq, PC, PCGadget> = ProofVar {
            commitments: commitment_gadgets,
            evaluations: evaluation_gadgets,
            prover_messages: prover_message_gadgets,
            pc_batch_proof,
        };
        // END: proof to proof_gadget

        MarlinVerificationGadget::<Fr, Fq, PC, PCGadget, MarlinRecursiveMode>::verify::<_, FS, FSG>(
            cs.ns(|| "marlin_verification"),
            &ivk_gadget,
            &public_input_gadget,
            &proof_gadget,
        )
        .unwrap()
        .enforce_equal(cs.ns(|| "enforce_equal"), &Boolean::Constant(true))
        .unwrap();

        assert!(
            cs.is_satisfied(),
            "Constraints not satisfied: {}",
            cs.which_is_unsatisfied().unwrap()
        );

        println!("cs - number of constraints: {}", cs.num_constraints());
        println!("cs - number of private variables: {}", cs.num_private_variables());
        println!("cs - number of public variables: {}", cs.num_public_variables());
    }
}
