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

use crate::{
    ahp::{matrices, prover::ProverConstraintSystem, verifier, AHPError, CircuitInfo},
    marlin::MarlinMode,
    String,
    ToString,
    Vec,
};
use snarkvm_algorithms::{cfg_iter_mut, fft::EvaluationDomain};
use snarkvm_fields::{batch_inversion, Field, PrimeField};
use snarkvm_r1cs::errors::SynthesisError;

use snarkvm_polycommit::{LCTerm, LabeledPolynomial, LinearCombination};

use core::{borrow::Borrow, marker::PhantomData};

#[cfg(feature = "parallel")]
use rayon::prelude::*;

/// The algebraic holographic proof defined in [CHMMVW19](https://eprint.iacr.org/2019/1047).
/// Currently, this AHP only supports inputs of size one
/// less than a power of 2 (i.e., of the form 2^n - 1).
pub struct AHPForR1CS<F: Field, MM: MarlinMode> {
    field: PhantomData<F>,
    mode: PhantomData<MM>,
}

impl<F: PrimeField, MM: MarlinMode> AHPForR1CS<F, MM> {
    /// The labels for the polynomials output by the AHP indexer.
    #[rustfmt::skip]
    pub const INDEXER_POLYNOMIALS: [&'static str; 6] = [
        // Polynomials for M
        "row", "col", "a_val", "b_val", "c_val", "row_col",
    ];
    /// The labels for the polynomials output and vanishing polynomials by the AHP indexer.
    #[rustfmt::skip]
    pub const INDEXER_POLYNOMIALS_WITH_VANISHING: [&'static str; 8] = [
        // Polynomials for M
        "row", "col", "a_val", "b_val", "c_val", "row_col",
        // Vanishing polynomials
        "vanishing_poly_h", "vanishing_poly_k"
    ];
    /// The linear combinations that are statically known to evaluate to zero.
    #[rustfmt::skip]
    pub const LC_WITH_ZERO_EVAL: [&'static str; 2] = ["inner_sumcheck", "outer_sumcheck"];
    /// The labels for the polynomials output by the AHP prover.
    #[rustfmt::skip]
    pub const PROVER_POLYNOMIALS_WITHOUT_ZK: [&'static str; 8] = [
        // First sumcheck
        "w", "z_a", "z_b", "t", "g_1", "h_1",
        // Second sumcheck
        "g_2", "h_2",
    ];
    /// The labels for the polynomials output by the AHP prover.
    #[rustfmt::skip]
    pub const PROVER_POLYNOMIALS_WITH_ZK: [&'static str; 9] = [
        // First sumcheck
        "w", "z_a", "z_b", "mask_poly", "t", "g_1", "h_1",
        // Second sumcheck
        "g_2", "h_2",
    ];

    pub(crate) fn indexer_polynomials() -> impl Iterator<Item = &'static str> {
        if MM::RECURSION {
            Self::INDEXER_POLYNOMIALS_WITH_VANISHING.as_ref().iter().copied()
        } else {
            Self::INDEXER_POLYNOMIALS.as_ref().iter().copied()
        }
    }

    pub(crate) fn prover_polynomials() -> impl Iterator<Item = &'static str> {
        if MM::ZK {
            Self::PROVER_POLYNOMIALS_WITH_ZK.as_ref().iter().copied()
        } else {
            Self::PROVER_POLYNOMIALS_WITHOUT_ZK.as_ref().iter().copied()
        }
    }

    pub(crate) fn polynomial_labels() -> impl Iterator<Item = String> {
        Self::indexer_polynomials()
            .chain(Self::prover_polynomials())
            .map(|s| s.to_string())
    }

    /// Check that the (formatted) public input is of the form 2^n for some integer n.
    pub fn num_formatted_public_inputs_is_admissible(num_inputs: usize) -> bool {
        num_inputs.count_ones() == 1
    }

    /// Check that the (formatted) public input is of the form 2^n for some integer n.
    pub fn formatted_public_input_is_admissible(input: &[F]) -> bool {
        Self::num_formatted_public_inputs_is_admissible(input.len())
    }

    /// The maximum degree of polynomials produced by the indexer and prover
    /// of this protocol.
    /// The number of the variables must include the "one" variable. That is, it
    /// must be with respect to the number of formatted public inputs.
    pub fn max_degree(num_constraints: usize, num_variables: usize, num_non_zero: usize) -> Result<usize, AHPError> {
        let padded_matrix_dim = matrices::padded_matrix_dim(num_variables, num_constraints);
        let zk_bound = 1;
        let domain_h_size = EvaluationDomain::<F>::compute_size_of_domain(padded_matrix_dim)
            .ok_or(SynthesisError::PolynomialDegreeTooLarge)?;
        let domain_k_size = EvaluationDomain::<F>::compute_size_of_domain(num_non_zero)
            .ok_or(SynthesisError::PolynomialDegreeTooLarge)?;

        Ok(*[
            2 * domain_h_size + zk_bound - 2,
            if MM::ZK {
                3 * domain_h_size + 2 * zk_bound - 3
            } else {
                0
            }, //  mask_poly
            domain_h_size,
            domain_h_size,
            domain_k_size - 1,
            domain_k_size, //  due to vanishing polynomial; for convenience, we increase the number by one regardless of the mode.
        ]
        .iter()
        .max()
        .unwrap())
    }

    /// Get all the strict degree bounds enforced in the AHP.
    pub fn get_degree_bounds(info: &CircuitInfo<F>) -> [usize; 2] {
        let mut degree_bounds = [0usize; 2];
        let num_constraints = info.num_constraints;
        let num_non_zero = info.num_non_zero;
        let h_size = EvaluationDomain::<F>::compute_size_of_domain(num_constraints).unwrap();
        let k_size = EvaluationDomain::<F>::compute_size_of_domain(num_non_zero).unwrap();

        degree_bounds[0] = h_size - 2;
        degree_bounds[1] = k_size - 2;
        degree_bounds
    }

    /// Construct the linear combinations that are checked by the AHP.
    /// Public input should be unformatted.
    #[allow(non_snake_case)]
    pub fn construct_linear_combinations<E: EvaluationsProvider<F>>(
        public_input: &[F],
        evals: &E,
        state: &verifier::VerifierState<F, MM>,
    ) -> Result<Vec<LinearCombination<F>>, AHPError> {
        let domain_h = state.domain_h;
        let domain_k = state.domain_k;
        let k_size = domain_k.size_as_field_element;

        let public_input = ProverConstraintSystem::format_public_input(public_input);
        if !Self::formatted_public_input_is_admissible(&public_input) {
            return Err(AHPError::InvalidPublicInputLength);
        }
        let x_domain = EvaluationDomain::new(public_input.len()).ok_or(SynthesisError::PolynomialDegreeTooLarge)?;

        let first_round_msg = state.first_round_message.unwrap();
        let alpha = first_round_msg.alpha;
        let eta_a = first_round_msg.eta_a;
        let eta_b = first_round_msg.eta_b;
        let eta_c = first_round_msg.eta_c;

        let beta = state.second_round_message.unwrap().beta;
        let gamma = state.gamma.unwrap();

        let mut linear_combinations = Vec::with_capacity(9);

        // Outer sumchecK:
        let z_b = LinearCombination::new("z_b", vec![(F::one(), "z_b")]);
        let g_1 = LinearCombination::new("g_1", vec![(F::one(), "g_1")]);
        let t = LinearCombination::new("t", vec![(F::one(), "t")]);

        let r_alpha_at_beta = domain_h.eval_unnormalized_bivariate_lagrange_poly(alpha, beta);
        let v_H_at_alpha = domain_h.evaluate_vanishing_polynomial(alpha);
        let v_H_at_beta = domain_h.evaluate_vanishing_polynomial(beta);
        let v_X_at_beta = x_domain.evaluate_vanishing_polynomial(beta);

        let z_b_at_beta = evals.get_lc_eval(&z_b, beta)?;
        let t_at_beta = evals.get_lc_eval(&t, beta)?;
        let g_1_at_beta = evals.get_lc_eval(&g_1, beta)?;

        let x_at_beta = x_domain
            .evaluate_all_lagrange_coefficients(beta)
            .into_iter()
            .zip(public_input)
            .map(|(l, x)| l * x)
            .fold(F::zero(), |x, y| x + y);

        #[rustfmt::skip]
        let outer_sumcheck = {
            let mut lc_terms = vec![];
            if MM::ZK {
                lc_terms.push((F::one(), "mask_poly".into()));
            }
            lc_terms.push((r_alpha_at_beta * (eta_a + (eta_c * z_b_at_beta)), "z_a".into()));
            lc_terms.push((r_alpha_at_beta * eta_b * z_b_at_beta, LCTerm::One));
            lc_terms.push((-t_at_beta * v_X_at_beta, "w".into()));
            lc_terms.push((-t_at_beta * x_at_beta, LCTerm::One));
            lc_terms.push((-v_H_at_beta, "h_1".into()));
            lc_terms.push((-beta * g_1_at_beta, LCTerm::One));
            LinearCombination::new("outer_sumcheck", lc_terms)
        };
        debug_assert!(evals.get_lc_eval(&outer_sumcheck, beta)?.is_zero());

        linear_combinations.push(z_b);
        linear_combinations.push(g_1);
        linear_combinations.push(t);
        linear_combinations.push(outer_sumcheck);

        //  Inner sumcheck:
        let beta_alpha = beta * alpha;
        let g_2 = LinearCombination::new("g_2", vec![(F::one(), "g_2")]);

        let g_2_at_gamma = evals.get_lc_eval(&g_2, gamma)?;

        let v_K_at_gamma = domain_k.evaluate_vanishing_polynomial(gamma);

        let mut a = LinearCombination::new("a_poly", vec![(eta_a, "a_val"), (eta_b, "b_val"), (eta_c, "c_val")]);
        a *= v_H_at_alpha * v_H_at_beta;

        let mut b = LinearCombination::new("denom", vec![
            (beta_alpha, LCTerm::One),
            (-alpha, "row".into()),
            (-beta, "col".into()),
            (F::one(), "row_col".into()),
        ]);
        b *= gamma * g_2_at_gamma + (t_at_beta / k_size);

        let mut inner_sumcheck = a;
        inner_sumcheck -= &b;
        inner_sumcheck -= &LinearCombination::new("h_2", vec![(v_K_at_gamma, "h_2")]);
        inner_sumcheck.label = "inner_sumcheck".into();
        debug_assert!(evals.get_lc_eval(&inner_sumcheck, gamma)?.is_zero());

        linear_combinations.push(g_2);
        linear_combinations.push(inner_sumcheck);

        if MM::RECURSION {
            let vanishing_poly_h_alpha =
                LinearCombination::new("vanishing_poly_h_alpha", vec![(F::one(), "vanishing_poly_h")]);
            let vanishing_poly_h_beta =
                LinearCombination::new("vanishing_poly_h_beta", vec![(F::one(), "vanishing_poly_h")]);
            let vanishing_poly_k_gamma =
                LinearCombination::new("vanishing_poly_k_gamma", vec![(F::one(), "vanishing_poly_k")]);

            linear_combinations.push(vanishing_poly_h_alpha);
            linear_combinations.push(vanishing_poly_h_beta);
            linear_combinations.push(vanishing_poly_k_gamma);
        }

        linear_combinations.sort_by(|a, b| a.label.cmp(&b.label));
        Ok(linear_combinations)
    }
}

/// Abstraction that provides evaluations of (linear combinations of) polynomials
///
/// Intended to provide a common interface for both the prover and the verifier
/// when constructing linear combinations via `AHPForR1CS::construct_linear_combinations`.
pub trait EvaluationsProvider<F: Field> {
    /// Get the evaluation of linear combination `lc` at `point`.
    fn get_lc_eval(&self, lc: &LinearCombination<F>, point: F) -> Result<F, AHPError>;
}

impl<'a, F: Field> EvaluationsProvider<F> for snarkvm_polycommit::Evaluations<'a, F> {
    fn get_lc_eval(&self, lc: &LinearCombination<F>, point: F) -> Result<F, AHPError> {
        let key = (lc.label.clone(), point);
        self.get(&key)
            .copied()
            .ok_or_else(|| AHPError::MissingEval(lc.label.clone()))
    }
}

impl<F: Field, T: Borrow<LabeledPolynomial<F>>> EvaluationsProvider<F> for Vec<T> {
    fn get_lc_eval(&self, lc: &LinearCombination<F>, point: F) -> Result<F, AHPError> {
        let mut eval = F::zero();
        for (coeff, term) in lc.iter() {
            let value = if let LCTerm::PolyLabel(label) = term {
                self.iter()
                    .find(|p| {
                        let p: &LabeledPolynomial<F> = (*p).borrow();
                        p.label() == label
                    })
                    .ok_or_else(|| AHPError::MissingEval(format!("Missing {} for {}", label, lc.label)))?
                    .borrow()
                    .evaluate(point)
            } else {
                assert!(term.is_one());
                F::one()
            };
            eval += &(*coeff * value)
        }
        Ok(eval)
    }
}

/// The derivative of the vanishing polynomial
pub trait UnnormalizedBivariateLagrangePoly<F: PrimeField> {
    /// Evaluate the polynomial
    fn eval_unnormalized_bivariate_lagrange_poly(&self, x: F, y: F) -> F;

    /// Evaluate over a batch of inputs
    fn batch_eval_unnormalized_bivariate_lagrange_poly_with_diff_inputs(&self, x: F) -> Vec<F>;

    /// Evaluate the magic polynomial over `self`
    fn batch_eval_unnormalized_bivariate_lagrange_poly_with_same_inputs(&self) -> Vec<F>;
}

impl<F: PrimeField> UnnormalizedBivariateLagrangePoly<F> for EvaluationDomain<F> {
    fn eval_unnormalized_bivariate_lagrange_poly(&self, x: F, y: F) -> F {
        if x != y {
            (self.evaluate_vanishing_polynomial(x) - self.evaluate_vanishing_polynomial(y)) / (x - y)
        } else {
            self.size_as_field_element * x.pow(&[(self.size() - 1) as u64])
        }
    }

    fn batch_eval_unnormalized_bivariate_lagrange_poly_with_diff_inputs(&self, x: F) -> Vec<F> {
        let vanish_x = self.evaluate_vanishing_polynomial(x);
        let mut inverses: Vec<F> = self.elements().map(|y| x - y).collect();
        batch_inversion(&mut inverses);

        cfg_iter_mut!(inverses).for_each(|denominator| *denominator *= &vanish_x);
        inverses
    }

    fn batch_eval_unnormalized_bivariate_lagrange_poly_with_same_inputs(&self) -> Vec<F> {
        let mut elems: Vec<F> = self.elements().map(|e| e * self.size_as_field_element).collect();
        elems[1..].reverse();
        elems
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snarkvm_algorithms::fft::{DenseOrSparsePolynomial, DensePolynomial};
    use snarkvm_curves::bls12_377::fr::Fr;
    use snarkvm_fields::{One, Zero};
    use snarkvm_utilities::rand::{test_rng, UniformRand};

    #[test]
    fn domain_unnormalized_bivariate_lagrange_poly() {
        for domain_size in 1..10 {
            let domain = EvaluationDomain::<Fr>::new(1 << domain_size).unwrap();
            let manual: Vec<_> = domain
                .elements()
                .map(|elem| domain.eval_unnormalized_bivariate_lagrange_poly(elem, elem))
                .collect();
            let fast = domain.batch_eval_unnormalized_bivariate_lagrange_poly_with_same_inputs();
            assert_eq!(fast, manual);
        }
    }

    #[test]
    fn domain_unnormalized_bivariate_lagrange_poly_diff_inputs() {
        let rng = &mut test_rng();
        for domain_size in 1..10 {
            let domain = EvaluationDomain::<Fr>::new(1 << domain_size).unwrap();
            let x = Fr::rand(rng);
            let manual: Vec<_> = domain
                .elements()
                .map(|y| domain.eval_unnormalized_bivariate_lagrange_poly(x, y))
                .collect();
            let fast = domain.batch_eval_unnormalized_bivariate_lagrange_poly_with_diff_inputs(x);
            assert_eq!(fast, manual);
        }
    }

    #[test]
    fn test_summation() {
        let rng = &mut test_rng();
        let size = 1 << 4;
        let domain = EvaluationDomain::<Fr>::new(1 << 4).unwrap();
        let size_as_fe = domain.size_as_field_element;
        let poly = DensePolynomial::rand(size, rng);

        let mut sum: Fr = Fr::zero();
        for eval in domain.elements().map(|e| poly.evaluate(e)) {
            sum += &eval;
        }
        let first = poly.coeffs[0] * size_as_fe;
        let last = *poly.coeffs.last().unwrap() * size_as_fe;
        println!("sum: {:?}", sum);
        println!("a_0: {:?}", first);
        println!("a_n: {:?}", last);
        println!("first + last: {:?}\n", first + last);
        assert_eq!(sum, first + last);
    }

    #[test]
    fn test_alternator_polynomial() {
        use snarkvm_algorithms::fft::Evaluations;
        let domain_k = EvaluationDomain::<Fr>::new(1 << 4).unwrap();
        let domain_h = EvaluationDomain::<Fr>::new(1 << 3).unwrap();
        let domain_h_elems = domain_h.elements().collect::<std::collections::HashSet<_>>();
        let alternator_poly_evals = domain_k
            .elements()
            .map(|e| {
                if domain_h_elems.contains(&e) {
                    Fr::one()
                } else {
                    Fr::zero()
                }
            })
            .collect();
        let v_k: DenseOrSparsePolynomial<_> = domain_k.vanishing_polynomial().into();
        let v_h: DenseOrSparsePolynomial<_> = domain_h.vanishing_polynomial().into();
        let (divisor, remainder) = v_k.divide_with_q_and_r(&v_h).unwrap();
        assert!(remainder.is_zero());
        println!("Divisor: {:?}", divisor);
        println!(
            "{:#?}",
            divisor
                .coeffs
                .iter()
                .filter_map(|f| if !f.is_zero() { Some(f.to_repr()) } else { None })
                .collect::<Vec<_>>()
        );

        for e in domain_h.elements() {
            println!("{:?}", divisor.evaluate(e));
        }
        // Let p = v_K / v_H;
        // The alternator polynomial is p * t, where t is defined as
        // the LDE of p(h)^{-1} for all h in H.
        //
        // Because for each h in H, p(h) equals a constant c, we have that t
        // is the constant polynomial c^{-1}.
        //
        // Q: what is the constant c? Why is p(h) constant? What is the easiest
        // way to calculate c?
        let alternator_poly = Evaluations::from_vec_and_domain(alternator_poly_evals, domain_k).interpolate();
        let (quotient, remainder) = DenseOrSparsePolynomial::from(alternator_poly.clone())
            .divide_with_q_and_r(&DenseOrSparsePolynomial::from(divisor))
            .unwrap();
        assert!(remainder.is_zero());
        println!("quotient: {:?}", quotient);
        println!(
            "{:#?}",
            quotient
                .coeffs
                .iter()
                .filter_map(|f| if !f.is_zero() { Some(f.to_repr()) } else { None })
                .collect::<Vec<_>>()
        );

        println!("{:?}", alternator_poly);
    }
}
