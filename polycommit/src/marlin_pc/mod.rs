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
    kzg10,
    BTreeMap,
    BTreeSet,
    BatchLCProof,
    Error,
    Evaluations,
    LabeledCommitment,
    LabeledPolynomial,
    LinearCombination,
    PCCommitterKey,
    PCRandomness,
    PCUniversalParams,
    Polynomial,
    PolynomialCommitment,
    QuerySet,
    String,
    ToOwned,
    ToString,
    Vec,
};
use snarkvm_curves::traits::{AffineCurve, PairingEngine, ProjectiveCurve};
use snarkvm_fields::{Field, One, Zero};

use core::{
    convert::TryInto,
    marker::PhantomData,
    ops::{Mul, MulAssign},
    sync::atomic::{AtomicBool, Ordering},
};
use rand_core::RngCore;

mod data_structures;
pub use data_structures::*;

mod gadgets;
pub use gadgets::*;

pub(crate) fn shift_polynomial<E: PairingEngine>(
    ck: &CommitterKey<E>,
    p: &Polynomial<E::Fr>,
    degree_bound: usize,
) -> Polynomial<E::Fr> {
    if p.is_zero() {
        Polynomial::zero()
    } else {
        let enforced_degree_bounds = ck
            .enforced_degree_bounds
            .as_ref()
            .expect("Polynomial requires degree bounds, but `ck` does not support any");
        let largest_enforced_degree_bound = enforced_degree_bounds.last().unwrap();

        let mut shifted_polynomial_coeffs = vec![E::Fr::zero(); largest_enforced_degree_bound - degree_bound];
        shifted_polynomial_coeffs.extend_from_slice(&p.coeffs);
        Polynomial::from_coefficients_vec(shifted_polynomial_coeffs)
    }
}

/// Polynomial commitment based on [[KZG10]][kzg], with degree enforcement, batching,
/// and (optional) hiding property taken from [[CHMMVW20, “Marlin”]][marlin].
///
/// Degree bound enforcement requires that (at least one of) the points at
/// which a committed polynomial is evaluated are from a distribution that is
/// random conditioned on the polynomial. This is because degree bound
/// enforcement relies on checking a polynomial identity at this point.
/// More formally, the points must be sampled from an admissible query sampler,
/// as detailed in [[CHMMVW20]][marlin].
///
/// [kzg]: http://cacr.uwaterloo.ca/techreports/2010/cacr2010-10.pdf
/// [marlin]: https://eprint.iacr.org/2019/104
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarlinKZG10<E: PairingEngine> {
    _engine: PhantomData<E>,
}

impl<E: PairingEngine> PolynomialCommitment<E::Fr, E::Fq> for MarlinKZG10<E> {
    type BatchProof = Vec<Self::Proof>;
    type Commitment = Commitment<E>;
    type CommitterKey = CommitterKey<E>;
    type PreparedCommitment = PreparedCommitment<E>;
    type PreparedVerifierKey = PreparedVerifierKey<E>;
    type Proof = kzg10::Proof<E>;
    type Randomness = Randomness<E>;
    type UniversalParams = UniversalParams<E>;
    type VerifierKey = VerifierKey<E>;

    /// Constructs public parameters when given as input the maximum degree `max_degree`
    /// for the polynomial commitment scheme.
    fn setup<R: RngCore>(max_degree: usize, rng: &mut R) -> Result<Self::UniversalParams, Error> {
        kzg10::KZG10::setup(max_degree, &kzg10::KZG10DegreeBoundsConfig::MARLIN, false, rng).map_err(Into::into)
    }

    /// Specializes the public parameters for polynomials up to the given `supported_degree`
    /// and for enforcing degree bounds in the range `1..=supported_degree`.
    fn trim(
        parameters: &Self::UniversalParams,
        supported_degree: usize,
        supported_hiding_bound: usize,
        enforced_degree_bounds: Option<&[usize]>,
    ) -> Result<(Self::CommitterKey, Self::VerifierKey), Error> {
        let max_degree = parameters.max_degree();
        if supported_degree > max_degree {
            return Err(Error::TrimmingDegreeTooLarge);
        }

        // Construct the KZG10 committer key for committing to unshifted polynomials.
        let ck_time =
            start_timer!(|| format!("Constructing `powers` of size {} for unshifted polys", supported_degree));
        let powers = parameters.powers_of_g[..=supported_degree].to_vec();
        // We want to support making up to `supported_hiding_bound` queries to committed polynomials.
        let powers_of_gamma_g = (0..=supported_hiding_bound + 1)
            .map(|i| parameters.powers_of_gamma_g[&i])
            .collect::<Vec<_>>();
        end_timer!(ck_time);

        // Construct the core KZG10 verifier key.
        let vk = kzg10::VerifierKey {
            g: parameters.powers_of_g[0],
            gamma_g: parameters.powers_of_gamma_g[&0],
            h: parameters.h,
            beta_h: parameters.beta_h,
            prepared_h: parameters.prepared_h.clone(),
            prepared_beta_h: parameters.prepared_beta_h.clone(),
        };

        let enforced_degree_bounds = enforced_degree_bounds.map(|v| {
            let mut v = v.to_vec();
            v.sort_unstable();
            v.dedup();
            v
        });

        // Check whether we have some degree bounds to enforce
        let shifted_powers = if let Some(enforced_degree_bounds) = enforced_degree_bounds.as_ref() {
            if enforced_degree_bounds.is_empty() {
                None
            } else {
                let mut sorted_enforced_degree_bounds = enforced_degree_bounds.clone();
                sorted_enforced_degree_bounds.sort_unstable();

                let lowest_shifted_power =
                    max_degree - sorted_enforced_degree_bounds.last().ok_or(Error::EmptyDegreeBounds)?;

                let shifted_ck_time = start_timer!(|| format!(
                    "Constructing `shifted_powers` of size {}",
                    max_degree - lowest_shifted_power + 1
                ));

                let shifted_powers = parameters.powers_of_g[lowest_shifted_power..].to_vec();
                end_timer!(shifted_ck_time);

                Some(shifted_powers)
            }
        } else {
            None
        };

        let degree_bounds_and_shift_powers = if parameters.inverse_powers_of_g.is_empty() {
            None
        } else {
            Some(
                parameters
                    .inverse_powers_of_g
                    .iter()
                    .map(|(d, affine)| (*d, *affine))
                    .collect::<Vec<(usize, E::G1Affine)>>(),
            )
        };

        let ck = CommitterKey {
            powers,
            shifted_powers,
            powers_of_gamma_g,
            enforced_degree_bounds,
            max_degree,
        };

        let vk = VerifierKey {
            vk,
            degree_bounds_and_shift_powers,
            supported_degree,
            max_degree,
        };
        Ok((ck, vk))
    }

    /// Outputs a commitment to `polynomial`.
    #[allow(clippy::type_complexity)]
    fn commit_with_terminator<'a>(
        ck: &Self::CommitterKey,
        polynomials: impl IntoIterator<Item = &'a LabeledPolynomial<E::Fr>>,
        terminator: &AtomicBool,
        rng: Option<&mut dyn RngCore>,
    ) -> Result<(Vec<LabeledCommitment<Self::Commitment>>, Vec<Self::Randomness>), Error> {
        let rng = &mut crate::optional_rng::OptionalRng(rng);
        let commit_time = start_timer!(|| "Committing to polynomials");

        let mut commitments = Vec::new();
        let mut randomness = Vec::new();

        for p in polynomials {
            let label = p.label();
            let degree_bound = p.degree_bound();
            let hiding_bound = p.hiding_bound();
            let polynomial = p.polynomial();

            if terminator.load(Ordering::Relaxed) {
                return Err(Error::Terminated);
            }

            let enforced_degree_bounds: Option<&[usize]> = ck.enforced_degree_bounds.as_deref();
            kzg10::KZG10::<E>::check_degrees_and_bounds(
                ck.supported_degree(),
                ck.max_degree,
                enforced_degree_bounds,
                p,
            )?;

            if terminator.load(Ordering::Relaxed) {
                return Err(Error::Terminated);
            }

            let commit_time = start_timer!(|| format!(
                "Polynomial {} of degree {}, degree bound {:?}, and hiding bound {:?}",
                label,
                polynomial.degree(),
                degree_bound,
                hiding_bound,
            ));

            let (comm, rand) = kzg10::KZG10::commit(&ck.powers(), polynomial, hiding_bound, terminator, Some(rng))?;
            let (shifted_comm, shifted_rand) = if let Some(degree_bound) = degree_bound {
                let shifted_powers = ck
                    .shifted_powers(degree_bound)
                    .ok_or(Error::UnsupportedDegreeBound(degree_bound))?;
                let (shifted_comm, shifted_rand) =
                    kzg10::KZG10::commit(&shifted_powers, polynomial, hiding_bound, terminator, Some(rng))?;
                (Some(shifted_comm), Some(shifted_rand))
            } else {
                (None, None)
            };

            let comm = Commitment { comm, shifted_comm };
            let rand = Randomness { rand, shifted_rand };
            commitments.push(LabeledCommitment::new(label.to_string(), comm, degree_bound));
            randomness.push(rand);
            end_timer!(commit_time);
        }
        end_timer!(commit_time);
        Ok((commitments, randomness))
    }

    /// On input a polynomial `p` and a point `point`, outputs a proof for the same.
    fn open<'a>(
        ck: &Self::CommitterKey,
        labeled_polynomials: impl IntoIterator<Item = &'a LabeledPolynomial<E::Fr>>,
        _commitments: impl IntoIterator<Item = &'a LabeledCommitment<Self::Commitment>>,
        point: E::Fr,
        opening_challenge: E::Fr,
        rands: impl IntoIterator<Item = &'a Self::Randomness>,
        _rng: Option<&mut dyn RngCore>,
    ) -> Result<Self::Proof, Error>
    where
        Self::Randomness: 'a,
        Self::Commitment: 'a,
    {
        let mut p = Polynomial::zero();
        let mut r = kzg10::Randomness::empty();
        let mut shifted_w = Polynomial::zero();
        let mut shifted_r = kzg10::Randomness::empty();
        let mut shifted_r_witness = Polynomial::zero();

        let mut enforce_degree_bound = false;
        for (j, (polynomial, rand)) in labeled_polynomials.into_iter().zip(rands).enumerate() {
            let degree_bound = polynomial.degree_bound();

            let enforced_degree_bounds: Option<&[usize]> = ck.enforced_degree_bounds.as_deref();
            kzg10::KZG10::<E>::check_degrees_and_bounds(
                ck.supported_degree(),
                ck.max_degree,
                enforced_degree_bounds,
                polynomial,
            )?;

            // compute challenge^j and challenge^{j+1}.
            let challenge_j = opening_challenge.pow([2 * j as u64]);

            assert_eq!(degree_bound.is_some(), rand.shifted_rand.is_some());

            p += (challenge_j, polynomial.polynomial());
            r += (challenge_j, &rand.rand);

            if let Some(degree_bound) = degree_bound {
                enforce_degree_bound = true;
                let shifted_rand = rand.shifted_rand.as_ref().unwrap();
                let (witness, shifted_rand_witness) =
                    kzg10::KZG10::compute_witness_polynomial(polynomial.polynomial(), point, shifted_rand)?;
                let challenge_j_1 = challenge_j * opening_challenge;

                let shifted_witness = shift_polynomial(ck, &witness, degree_bound);

                shifted_w += (challenge_j_1, &shifted_witness);
                shifted_r += (challenge_j_1, shifted_rand);
                if let Some(shifted_rand_witness) = shifted_rand_witness {
                    shifted_r_witness += (challenge_j_1, &shifted_rand_witness);
                }
            }
        }
        let proof_time = start_timer!(|| "Creating proof for unshifted polynomials");
        let proof = kzg10::KZG10::open(&ck.powers(), &p, point, &r)?;
        let mut w = proof.w.into_projective();
        let mut random_v = proof.random_v;
        end_timer!(proof_time);

        if enforce_degree_bound {
            let proof_time = start_timer!(|| "Creating proof for shifted polynomials");
            let shifted_proof = kzg10::KZG10::open_with_witness_polynomial(
                &ck.shifted_powers(None).unwrap(),
                point,
                &shifted_r,
                &shifted_w,
                Some(&shifted_r_witness),
            )?;
            end_timer!(proof_time);

            w += &shifted_proof.w.into_projective();
            if let Some(shifted_random_v) = shifted_proof.random_v {
                random_v = random_v.map(|v| v + shifted_random_v);
            }
        }

        Ok(kzg10::Proof {
            w: w.into_affine(),
            random_v,
        })
    }

    /// Verifies that `value` is the evaluation at `x` of the polynomial
    /// committed inside `comm`.
    fn check<'a, R: RngCore>(
        vk: &Self::VerifierKey,
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Self::Commitment>>,
        point: E::Fr,
        values: impl IntoIterator<Item = E::Fr>,
        proof: &Self::Proof,
        opening_challenge: E::Fr,
        _rng: &mut R,
    ) -> Result<bool, Error>
    where
        Self::Commitment: 'a,
    {
        let check_time = start_timer!(|| "Checking evaluations");
        let (combined_comm, combined_value) =
            Self::accumulate_commitments_and_values(vk, commitments, values, opening_challenge)?;
        let combined_comm = kzg10::Commitment(combined_comm.into());
        let result = kzg10::KZG10::check(&vk.vk, &combined_comm, point, combined_value, proof)?;
        end_timer!(check_time);
        Ok(result)
    }

    fn batch_check<'a, R: RngCore>(
        vk: &Self::VerifierKey,
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Self::Commitment>>,
        query_set: &QuerySet<E::Fr>,
        values: &Evaluations<E::Fr>,
        proof: &Self::BatchProof,
        opening_challenge: E::Fr,
        rng: &mut R,
    ) -> Result<bool, Error>
    where
        Self::Commitment: 'a,
    {
        let commitments: BTreeMap<_, _> = commitments.into_iter().map(|c| (c.label().to_owned(), c)).collect();
        let mut query_to_labels_map = BTreeMap::new();

        for (label, (point_name, point)) in query_set.iter() {
            let labels = query_to_labels_map
                .entry(point_name)
                .or_insert((point, BTreeSet::new()));
            labels.1.insert(label);
        }
        assert_eq!(proof.len(), query_to_labels_map.len());

        let mut combined_comms = Vec::with_capacity(query_to_labels_map.len());
        let mut combined_queries = Vec::with_capacity(query_to_labels_map.len());
        let mut combined_evals = Vec::with_capacity(query_to_labels_map.len());
        for (_point_name, (query, labels)) in query_to_labels_map.into_iter() {
            let lc_time = start_timer!(|| format!("Randomly combining {} commitments", labels.len()));
            let mut comms_to_combine = Vec::with_capacity(labels.len());
            let mut values_to_combine = Vec::with_capacity(labels.len());
            for label in labels.into_iter() {
                let commitment = commitments.get(label).ok_or(Error::MissingPolynomial {
                    label: label.to_string(),
                })?;
                let degree_bound = commitment.degree_bound();
                assert_eq!(degree_bound.is_some(), commitment.commitment().shifted_comm.is_some());

                let v_i = values.get(&(label.clone(), *query)).ok_or(Error::MissingEvaluation {
                    label: label.to_string(),
                })?;

                comms_to_combine.push(*commitment);
                values_to_combine.push(*v_i);
            }
            let (c, v) =
                Self::accumulate_commitments_and_values(vk, comms_to_combine, values_to_combine, opening_challenge)?;
            end_timer!(lc_time);
            combined_comms.push(c);
            combined_queries.push(*query);
            combined_evals.push(v);
        }
        let norm_time = start_timer!(|| "Normalizaing combined commitments");
        E::G1Projective::batch_normalization(&mut combined_comms);
        let combined_comms: Vec<_> = combined_comms
            .into_iter()
            .map(|c| kzg10::Commitment(c.into()))
            .collect();
        end_timer!(norm_time);
        let proof_time = start_timer!(|| "Checking KZG10::Proof");
        let result =
            kzg10::KZG10::batch_check(&vk.vk, &combined_comms, &combined_queries, &combined_evals, proof, rng)?;
        end_timer!(proof_time);
        Ok(result)
    }

    fn open_combinations<'a>(
        ck: &Self::CommitterKey,
        lc_s: impl IntoIterator<Item = &'a LinearCombination<E::Fr>>,
        polynomials: impl IntoIterator<Item = &'a LabeledPolynomial<E::Fr>>,
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Self::Commitment>>,
        query_set: &QuerySet<E::Fr>,
        opening_challenge: E::Fr,
        rands: impl IntoIterator<Item = &'a Self::Randomness>,
        rng: Option<&mut dyn RngCore>,
    ) -> Result<BatchLCProof<E::Fr, E::Fq, Self>, Error>
    where
        Self::Randomness: 'a,
        Self::Commitment: 'a,
    {
        let label_map = polynomials
            .into_iter()
            .zip(rands)
            .zip(commitments)
            .map(|((p, r), c)| (p.label(), (p, r, c)))
            .collect::<BTreeMap<_, _>>();

        let mut lc_polynomials = Vec::new();
        let mut lc_randomness = Vec::new();
        let mut lc_commitments = Vec::new();
        let mut lc_info = Vec::new();

        for lc in lc_s {
            let lc_label = lc.label().clone();
            let mut poly = Polynomial::zero();
            let mut degree_bound = None;
            let mut hiding_bound = None;

            let mut randomness = Self::Randomness::empty();
            assert!(randomness.shifted_rand.is_none());

            let mut coeffs_and_comms = Vec::new();

            let num_polys = lc.len();
            for (coeff, label) in lc.iter().filter(|(_, l)| !l.is_one()) {
                let label: &String = label.try_into().expect("cannot be one!");
                let &(cur_poly, cur_rand, cur_comm) = label_map.get(label).ok_or(Error::MissingPolynomial {
                    label: label.to_string(),
                })?;

                if num_polys == 1 && cur_poly.degree_bound().is_some() {
                    assert!(coeff.is_one(), "Coefficient must be one for degree-bounded equations");
                    degree_bound = cur_poly.degree_bound();
                } else if cur_poly.degree_bound().is_some() {
                    eprintln!("Degree bound when number of equations is non-zero");
                    return Err(Error::EquationHasDegreeBounds(lc_label));
                }

                // Some(_) > None, always.
                hiding_bound = core::cmp::max(hiding_bound, cur_poly.hiding_bound());
                poly += (*coeff, cur_poly.polynomial());
                randomness += (*coeff, cur_rand);
                coeffs_and_comms.push((*coeff, cur_comm.commitment()));

                if degree_bound.is_none() {
                    assert!(randomness.shifted_rand.is_none());
                }
            }

            let lc_poly = LabeledPolynomial::new(lc_label.clone(), poly, degree_bound, hiding_bound);
            lc_polynomials.push(lc_poly);
            lc_randomness.push(randomness);
            lc_commitments.push(Self::combine_commitments(coeffs_and_comms));
            lc_info.push((lc_label, degree_bound));
        }

        let comms = Self::normalize_commitments(lc_commitments);
        let lc_commitments = lc_info
            .into_iter()
            .zip(comms)
            .map(|((label, d), c)| LabeledCommitment::new(label, c, d))
            .collect::<Vec<_>>();

        let proof = Self::batch_open(
            ck,
            lc_polynomials.iter(),
            lc_commitments.iter(),
            query_set,
            opening_challenge,
            lc_randomness.iter(),
            rng,
        )?;

        Ok(BatchLCProof {
            proof,
            evaluations: None,
        })
    }

    /// Checks that `values` are the true evaluations at `query_set` of the polynomials
    /// committed in `labeled_commitments`.
    fn check_combinations<'a, R: RngCore>(
        vk: &Self::VerifierKey,
        lc_s: impl IntoIterator<Item = &'a LinearCombination<E::Fr>>,
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Self::Commitment>>,
        query_set: &QuerySet<E::Fr>,
        evaluations: &Evaluations<E::Fr>,
        proof: &BatchLCProof<E::Fr, E::Fq, Self>,
        opening_challenge: E::Fr,
        rng: &mut R,
    ) -> Result<bool, Error>
    where
        Self::Commitment: 'a,
    {
        let BatchLCProof { proof, .. } = proof;
        let label_comm_map = commitments
            .into_iter()
            .map(|c| (c.label().to_owned(), c))
            .collect::<BTreeMap<_, _>>();

        let mut lc_commitments = Vec::new();
        let mut lc_info = Vec::new();
        let mut evaluations = evaluations.clone();

        let lc_processing_time = start_timer!(|| "Combining commitments");
        for lc in lc_s {
            let lc_label = lc.label().clone();
            let num_polys = lc.len();

            let mut degree_bound = None;
            let mut coeffs_and_comms = Vec::new();

            for (coeff, label) in lc.iter() {
                if label.is_one() {
                    for (&(ref label, _), ref mut eval) in evaluations.iter_mut() {
                        if label == &lc_label {
                            **eval -= coeff;
                        }
                    }
                } else {
                    let label: String = label.to_owned().try_into().unwrap();
                    let cur_comm = label_comm_map.get(&label).ok_or(Error::MissingPolynomial {
                        label: label.to_string(),
                    })?;

                    if num_polys == 1 && cur_comm.degree_bound().is_some() {
                        assert!(coeff.is_one(), "Coefficient must be one for degree-bounded equations");
                        degree_bound = cur_comm.degree_bound();
                    } else if cur_comm.degree_bound().is_some() {
                        return Err(Error::EquationHasDegreeBounds(lc_label));
                    }
                    coeffs_and_comms.push((*coeff, cur_comm.commitment()));
                }
            }
            let lc_time = start_timer!(|| format!("Combining {} commitments for {}", num_polys, lc_label));
            lc_commitments.push(Self::combine_commitments(coeffs_and_comms));
            end_timer!(lc_time);
            lc_info.push((lc_label, degree_bound));
        }
        end_timer!(lc_processing_time);
        let combined_comms_norm_time = start_timer!(|| "Normalizing commitments");
        let comms = Self::normalize_commitments(lc_commitments);
        let lc_commitments: Vec<_> = lc_info
            .into_iter()
            .zip(comms)
            .map(|((label, d), c)| LabeledCommitment::new(label, c, d))
            .collect();
        end_timer!(combined_comms_norm_time);

        Self::batch_check(
            vk,
            &lc_commitments,
            query_set,
            &evaluations,
            proof,
            opening_challenge,
            rng,
        )
    }

    /// On input a list of polynomials, linear combinations of those polynomials,
    /// and a query set, `open_combination` outputs a proof of evaluation of
    /// the combinations at the points in the query set.
    fn open_combinations_individual_opening_challenges<'a>(
        ck: &Self::CommitterKey,
        linear_combinations: impl IntoIterator<Item = &'a LinearCombination<E::Fr>>,
        polynomials: impl IntoIterator<Item = &'a LabeledPolynomial<E::Fr>>,
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Self::Commitment>>,
        query_set: &QuerySet<E::Fr>,
        opening_challenges: &dyn Fn(u64) -> E::Fr,
        rands: impl IntoIterator<Item = &'a Self::Randomness>,
    ) -> Result<BatchLCProof<E::Fr, E::Fq, Self>, Error>
    where
        Self::Randomness: 'a,
        Self::Commitment: 'a,
    {
        let label_map = polynomials
            .into_iter()
            .zip(rands)
            .zip(commitments)
            .map(|((p, r), c)| (p.label(), (p, r, c)))
            .collect::<BTreeMap<_, _>>();

        let mut lc_polynomials = Vec::new();
        let mut lc_randomness = Vec::new();
        let mut lc_commitments = Vec::new();
        let mut lc_info = Vec::new();

        for lc in linear_combinations {
            let lc_label = lc.label().clone();
            let mut poly = Polynomial::zero();
            let mut degree_bound = None;
            let mut hiding_bound = None;

            let mut randomness = Self::Randomness::empty();
            let mut coeffs_and_comms = Vec::new();

            let num_polys = lc.len();
            for (coeff, label) in lc.iter().filter(|(_, l)| !l.is_one()) {
                let label: &String = label.try_into().expect("cannot be one!");
                let &(cur_poly, cur_rand, cur_comm) = label_map.get(label).ok_or(Error::MissingPolynomial {
                    label: label.to_string(),
                })?;
                if num_polys == 1 && cur_poly.degree_bound().is_some() {
                    assert!(coeff.is_one(), "Coefficient must be one for degree-bounded equations");
                    degree_bound = cur_poly.degree_bound();
                } else if cur_poly.degree_bound().is_some() {
                    return Err(Error::EquationHasDegreeBounds(lc_label));
                }
                // Some(_) > None, always.
                hiding_bound = core::cmp::max(hiding_bound, cur_poly.hiding_bound());
                poly += (*coeff, cur_poly.polynomial());
                randomness += (*coeff, cur_rand);
                coeffs_and_comms.push((*coeff, cur_comm.commitment()));
            }

            let lc_poly = LabeledPolynomial::new(lc_label.clone(), poly, degree_bound, hiding_bound);
            lc_polynomials.push(lc_poly);
            lc_randomness.push(randomness);
            lc_commitments.push(Self::combine_commitments(coeffs_and_comms));
            lc_info.push((lc_label, degree_bound));
        }

        let comms = Self::normalize_commitments(lc_commitments);
        let lc_commitments = lc_info
            .into_iter()
            .zip(comms)
            .map(|((label, d), c)| LabeledCommitment::new(label, c, d))
            .collect::<Vec<_>>();

        let proof = Self::batch_open_individual_opening_challenges(
            ck,
            lc_polynomials.iter(),
            lc_commitments.iter(),
            query_set,
            opening_challenges,
            lc_randomness.iter(),
        )?;

        Ok(BatchLCProof {
            proof,
            evaluations: None,
        })
    }

    /// Check combinations with individual challenges.
    fn check_combinations_individual_opening_challenges<'a, R: RngCore>(
        vk: &Self::VerifierKey,
        linear_combinations: impl IntoIterator<Item = &'a LinearCombination<E::Fr>>,
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Self::Commitment>>,
        query_set: &QuerySet<E::Fr>,
        evaluations: &Evaluations<E::Fr>,
        proof: &BatchLCProof<E::Fr, E::Fq, Self>,
        opening_challenges: &dyn Fn(u64) -> E::Fr,
        rng: &mut R,
    ) -> Result<bool, Error>
    where
        Self::Commitment: 'a,
    {
        let BatchLCProof { proof, .. } = proof;
        let label_comm_map = commitments
            .into_iter()
            .map(|c| (c.label(), c))
            .collect::<BTreeMap<_, _>>();

        let mut lc_commitments = Vec::new();
        let mut lc_info = Vec::new();
        let mut evaluations = evaluations.clone();

        let lc_processing_time = start_timer!(|| "Combining commitments");
        for lc in linear_combinations {
            let lc_label = lc.label().clone();
            let num_polys = lc.len();

            let mut degree_bound = None;
            let mut coeffs_and_comms = Vec::new();

            for (coeff, label) in lc.iter() {
                if label.is_one() {
                    for (&(ref label, _), ref mut eval) in evaluations.iter_mut() {
                        if label == &lc_label {
                            **eval -= coeff;
                        }
                    }
                } else {
                    let label: &String = label.try_into().unwrap();
                    let &cur_comm = label_comm_map.get(label).ok_or(Error::MissingPolynomial {
                        label: label.to_string(),
                    })?;

                    if num_polys == 1 && cur_comm.degree_bound().is_some() {
                        assert!(coeff.is_one(), "Coefficient must be one for degree-bounded equations");
                        degree_bound = cur_comm.degree_bound();
                    } else if cur_comm.degree_bound().is_some() {
                        return Err(Error::EquationHasDegreeBounds(lc_label));
                    }
                    coeffs_and_comms.push((*coeff, cur_comm.commitment()));
                }
            }
            let lc_time = start_timer!(|| format!("Combining {} commitments for {}", num_polys, lc_label));
            lc_commitments.push(Self::combine_commitments(coeffs_and_comms));
            end_timer!(lc_time);
            lc_info.push((lc_label, degree_bound));
        }
        end_timer!(lc_processing_time);
        let combined_comms_norm_time = start_timer!(|| "Normalizing commitments");
        let comms = Self::normalize_commitments(lc_commitments);
        let lc_commitments = lc_info
            .into_iter()
            .zip(comms)
            .map(|((label, d), c)| LabeledCommitment::new(label, c, d))
            .collect::<Vec<_>>();
        end_timer!(combined_comms_norm_time);

        Self::batch_check_individual_opening_challenges(
            vk,
            &lc_commitments,
            query_set,
            &evaluations,
            proof,
            opening_challenges,
            rng,
        )
    }
}

impl<E: PairingEngine> MarlinKZG10<E> {
    /// On input a polynomial `p` and a point `point`, outputs a proof for the same.
    fn open_individual_opening_challenges<'a>(
        ck: &<Self as PolynomialCommitment<E::Fr, E::Fq>>::CommitterKey,
        labeled_polynomials: impl IntoIterator<Item = &'a LabeledPolynomial<E::Fr>>,
        _commitments: impl IntoIterator<
            Item = &'a LabeledCommitment<<Self as PolynomialCommitment<E::Fr, E::Fq>>::Commitment>,
        >,
        point: &'a E::Fr,
        opening_challenges: &dyn Fn(u64) -> E::Fr,
        rands: impl IntoIterator<Item = &'a <Self as PolynomialCommitment<E::Fr, E::Fq>>::Randomness>,
    ) -> Result<<Self as PolynomialCommitment<E::Fr, E::Fq>>::Proof, Error>
    where
        <Self as PolynomialCommitment<E::Fr, E::Fq>>::Randomness: 'a,
        <Self as PolynomialCommitment<E::Fr, E::Fq>>::Commitment: 'a,
    {
        let mut p = Polynomial::zero();
        let mut r = kzg10::Randomness::empty();
        let mut shifted_w = Polynomial::zero();
        let mut shifted_r = kzg10::Randomness::empty();
        let mut shifted_r_witness = Polynomial::zero();

        let mut enforce_degree_bound = false;
        let mut opening_challenge_counter = 0;

        for (polynomial, rand) in labeled_polynomials.into_iter().zip(rands) {
            let degree_bound = polynomial.degree_bound();
            assert_eq!(degree_bound.is_some(), rand.shifted_rand.is_some());

            let enforced_degree_bounds: Option<&[usize]> = ck.enforced_degree_bounds.as_deref();
            kzg10::KZG10::<E>::check_degrees_and_bounds(
                ck.supported_degree(),
                ck.max_degree,
                enforced_degree_bounds,
                polynomial,
            )?;

            // compute challenge^j and challenge^{j+1}.

            let challenge_j = opening_challenges(opening_challenge_counter);
            opening_challenge_counter += 1;

            assert_eq!(degree_bound.is_some(), rand.shifted_rand.is_some());

            p += (challenge_j, polynomial.polynomial());
            r += (challenge_j, &rand.rand);

            if let Some(degree_bound) = degree_bound {
                enforce_degree_bound = true;
                let shifted_rand = rand.shifted_rand.as_ref().unwrap();
                let (witness, shifted_rand_witness) =
                    kzg10::KZG10::<E>::compute_witness_polynomial(polynomial.polynomial(), *point, shifted_rand)?;

                let challenge_j_1 = opening_challenges(opening_challenge_counter);
                opening_challenge_counter += 1;

                let shifted_witness = shift_polynomial(ck, &witness, degree_bound);

                shifted_w += (challenge_j_1, &shifted_witness);
                shifted_r += (challenge_j_1, shifted_rand);
                if let Some(shifted_rand_witness) = shifted_rand_witness {
                    shifted_r_witness += (challenge_j_1, &shifted_rand_witness);
                }
            }
        }
        let proof_time = start_timer!(|| "Creating proof for unshifted polynomials");
        let proof = kzg10::KZG10::open(&ck.powers(), &p, *point, &r)?;
        let mut w = proof.w.into_projective();
        let mut random_v = proof.random_v;
        end_timer!(proof_time);

        if enforce_degree_bound {
            let proof_time = start_timer!(|| "Creating proof for shifted polynomials");
            let shifted_proof = kzg10::KZG10::open_with_witness_polynomial(
                &ck.shifted_powers(None).unwrap(),
                *point,
                &shifted_r,
                &shifted_w,
                Some(&shifted_r_witness),
            )?;
            end_timer!(proof_time);

            w += &shifted_proof.w.into_projective();
            if let Some(shifted_random_v) = shifted_proof.random_v {
                random_v = random_v.map(|v| v + shifted_random_v);
            }
        }

        Ok(kzg10::Proof {
            w: w.into_affine(),
            random_v,
        })
    }

    /// On input a list of labeled polynomials and a query set, `open` outputs a proof of evaluation
    /// of the polynomials at the points in the query set.
    fn batch_open_individual_opening_challenges<'a>(
        ck: &<Self as PolynomialCommitment<E::Fr, E::Fq>>::CommitterKey,
        labeled_polynomials: impl IntoIterator<Item = &'a LabeledPolynomial<E::Fr>>,
        commitments: impl IntoIterator<
            Item = &'a LabeledCommitment<<Self as PolynomialCommitment<E::Fr, E::Fq>>::Commitment>,
        >,
        query_set: &QuerySet<E::Fr>,
        opening_challenges: &dyn Fn(u64) -> E::Fr,
        rands: impl IntoIterator<Item = &'a <Self as PolynomialCommitment<E::Fr, E::Fq>>::Randomness>,
    ) -> Result<<Self as PolynomialCommitment<E::Fr, E::Fq>>::BatchProof, Error>
    where
        <Self as PolynomialCommitment<E::Fr, E::Fq>>::Randomness: 'a,
        <Self as PolynomialCommitment<E::Fr, E::Fq>>::Commitment: 'a,
    {
        let poly_rand_comm: BTreeMap<_, _> = labeled_polynomials
            .into_iter()
            .zip(rands)
            .zip(commitments.into_iter())
            .map(|((poly, r), comm)| (poly.label(), (poly, r, comm)))
            .collect();

        let open_time = start_timer!(|| format!(
            "Opening {} polynomials at query set of size {}",
            poly_rand_comm.len(),
            query_set.len(),
        ));

        let mut query_to_labels_map = BTreeMap::new();

        for (label, (point_name, point)) in query_set.iter() {
            let labels = query_to_labels_map
                .entry(point_name)
                .or_insert((point, BTreeSet::new()));
            labels.1.insert(label);
        }

        // As a result of BTreeMap, the query is sorted by names of the points.

        let mut proofs = Vec::new();
        for (_point_name, (query, labels)) in query_to_labels_map.into_iter() {
            let mut query_polys: Vec<&'a LabeledPolynomial<_>> = Vec::new();
            let mut query_rands: Vec<&'a <Self as PolynomialCommitment<E::Fr, E::Fq>>::Randomness> = Vec::new();
            let mut query_comms: Vec<&'a LabeledCommitment<<Self as PolynomialCommitment<E::Fr, E::Fq>>::Commitment>> =
                Vec::new();

            for label in labels {
                let (polynomial, rand, comm) = poly_rand_comm.get(&label).ok_or(Error::MissingPolynomial {
                    label: label.to_string(),
                })?;

                query_polys.push(polynomial);
                query_rands.push(rand);
                query_comms.push(comm);
            }

            let proof_time = start_timer!(|| "Creating proof");
            let proof = Self::open_individual_opening_challenges(
                ck,
                query_polys,
                query_comms,
                query,
                opening_challenges,
                query_rands,
            )?;

            end_timer!(proof_time);

            proofs.push(proof);
        }
        end_timer!(open_time);

        Ok(proofs)
    }

    /// Verifies that `value` is the evaluation at `x` of the polynomial
    /// committed inside `comm`.
    #[allow(dead_code)]
    fn check_individual_opening_challenges<'a>(
        vk: &<Self as PolynomialCommitment<E::Fr, E::Fq>>::VerifierKey,
        commitments: impl IntoIterator<
            Item = &'a LabeledCommitment<<Self as PolynomialCommitment<E::Fr, E::Fq>>::Commitment>,
        >,
        point: &'a E::Fr,
        values: impl IntoIterator<Item = E::Fr>,
        proof: &<Self as PolynomialCommitment<E::Fr, E::Fq>>::Proof,
        opening_challenges: &dyn Fn(u64) -> E::Fr,
    ) -> Result<bool, Error>
    where
        <Self as PolynomialCommitment<E::Fr, E::Fq>>::Commitment: 'a,
    {
        let check_time = start_timer!(|| "Checking evaluations");
        let (combined_comm, combined_value) = Self::accumulate_commitments_and_values_individual_opening_challenges(
            vk,
            commitments,
            values,
            opening_challenges,
        )?;
        let combined_comm = kzg10::Commitment(combined_comm.into());
        let result = kzg10::KZG10::check(&vk.vk, &combined_comm, *point, combined_value, proof)?;
        end_timer!(check_time);
        Ok(result)
    }

    /// batch_check but with individual challenges
    fn batch_check_individual_opening_challenges<'a, R: RngCore>(
        vk: &<Self as PolynomialCommitment<E::Fr, E::Fq>>::VerifierKey,
        commitments: impl IntoIterator<
            Item = &'a LabeledCommitment<<Self as PolynomialCommitment<E::Fr, E::Fq>>::Commitment>,
        >,
        query_set: &QuerySet<E::Fr>,
        values: &Evaluations<E::Fr>,
        proof: &<Self as PolynomialCommitment<E::Fr, E::Fq>>::BatchProof,
        opening_challenges: &dyn Fn(u64) -> E::Fr,
        rng: &mut R,
    ) -> Result<bool, Error>
    where
        <Self as PolynomialCommitment<E::Fr, E::Fq>>::Commitment: 'a,
    {
        let (combined_comms, combined_queries, combined_evals) =
            Self::combine_and_normalize(commitments, query_set, values, opening_challenges, vk)?;
        assert_eq!(proof.len(), combined_queries.len());
        let proof_time = start_timer!(|| "Checking KZG10::Proof");
        let result =
            kzg10::KZG10::batch_check(&vk.vk, &combined_comms, &combined_queries, &combined_evals, proof, rng)?;
        end_timer!(proof_time);
        Ok(result)
    }
}

impl<E: PairingEngine> MarlinKZG10<E> {
    /// MSM for `commitments` and `coeffs`
    fn combine_commitments<'a>(
        coeffs_and_comms: impl IntoIterator<Item = (E::Fr, &'a Commitment<E>)>,
    ) -> (E::G1Projective, Option<E::G1Projective>) {
        let mut combined_comm = E::G1Projective::zero();
        let mut combined_shifted_comm = None;
        for (coeff, comm) in coeffs_and_comms {
            if coeff.is_one() {
                combined_comm.add_assign_mixed(&comm.comm.0);
            } else {
                combined_comm += &comm.comm.0.mul(coeff).into();
            }

            if let Some(shifted_comm) = &comm.shifted_comm {
                let cur = shifted_comm.0.mul(coeff).into_projective();
                combined_shifted_comm = Some(combined_shifted_comm.map_or(cur, |c| c + cur));
            }
        }
        (combined_comm, combined_shifted_comm)
    }

    fn normalize_commitments(
        commitments: Vec<(E::G1Projective, Option<E::G1Projective>)>,
    ) -> impl Iterator<Item = Commitment<E>> {
        let mut comms = Vec::with_capacity(commitments.len());
        let mut s_comms = Vec::with_capacity(commitments.len());
        let mut s_flags = Vec::with_capacity(commitments.len());
        for (comm, s_comm) in commitments {
            comms.push(comm);
            if let Some(c) = s_comm {
                s_comms.push(c);
                s_flags.push(true);
            } else {
                s_comms.push(E::G1Projective::zero());
                s_flags.push(false);
            }
        }
        let comms = E::G1Projective::batch_normalization_into_affine(comms);
        let s_comms = E::G1Projective::batch_normalization_into_affine(s_comms);
        comms.into_iter().zip(s_comms).zip(s_flags).map(|((c, s_c), flag)| {
            let shifted_comm = if flag { Some(kzg10::Commitment(s_c)) } else { None };
            Commitment {
                comm: kzg10::Commitment(c),
                shifted_comm,
            }
        })
    }

    /// Combine and normalize a set of commitments
    fn combine_and_normalize<'a>(
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Commitment<E>>>,
        query_set: &QuerySet<E::Fr>,
        evaluations: &Evaluations<E::Fr>,
        opening_challenges: &dyn Fn(u64) -> E::Fr,
        vk: &VerifierKey<E>,
    ) -> Result<(Vec<kzg10::Commitment<E>>, Vec<E::Fr>, Vec<E::Fr>), Error>
    where
        Commitment<E>: 'a,
    {
        let commitments: BTreeMap<_, _> = commitments.into_iter().map(|c| (c.label(), c)).collect();
        let mut query_to_labels_map = BTreeMap::new();

        for (label, (point_name, point)) in query_set.iter() {
            let labels = query_to_labels_map
                .entry(point_name)
                .or_insert((point, BTreeSet::new()));
            labels.1.insert(label);
        }

        let mut combined_comms = Vec::new();
        let mut combined_queries = Vec::new();
        let mut combined_evals = Vec::new();
        for (_point_name, (point, labels)) in query_to_labels_map.into_iter() {
            let lc_time = start_timer!(|| format!("Randomly combining {} commitments", labels.len()));
            let mut comms_to_combine: Vec<&'_ LabeledCommitment<_>> = Vec::new();
            let mut values_to_combine = Vec::new();
            for label in labels.into_iter() {
                let commitment = commitments.get(label).ok_or(Error::MissingPolynomial {
                    label: label.to_string(),
                })?;
                let degree_bound = commitment.degree_bound();
                assert_eq!(degree_bound.is_some(), commitment.commitment().shifted_comm.is_some());

                let v_i = evaluations
                    .get(&(label.clone(), *point))
                    .ok_or(Error::MissingEvaluation {
                        label: label.to_string(),
                    })?;

                comms_to_combine.push(commitment);
                values_to_combine.push(*v_i);
            }

            let (c, v) = Self::accumulate_commitments_and_values_individual_opening_challenges(
                vk,
                comms_to_combine,
                values_to_combine,
                opening_challenges,
            )?;
            end_timer!(lc_time);

            combined_comms.push(c);
            combined_queries.push(*point);
            combined_evals.push(v);
        }
        let norm_time = start_timer!(|| "Normalizing combined commitments");
        E::G1Projective::batch_normalization(&mut combined_comms);
        let combined_comms = combined_comms
            .into_iter()
            .map(|c| kzg10::Commitment(c.into()))
            .collect::<Vec<_>>();
        end_timer!(norm_time);
        Ok((combined_comms, combined_queries, combined_evals))
    }

    /// Accumulate `commitments` and `values` according to `opening_challenge`.
    fn accumulate_commitments_and_values<'a>(
        vk: &VerifierKey<E>,
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Commitment<E>>>,
        values: impl IntoIterator<Item = E::Fr>,
        opening_challenge: E::Fr,
    ) -> Result<(E::G1Projective, E::Fr), Error> {
        let acc_time = start_timer!(|| "Accumulating commitments and values");
        let mut combined_comm = E::G1Projective::zero();
        let mut combined_value = E::Fr::zero();
        let mut challenge_i = E::Fr::one();
        for (labeled_commitment, value) in commitments.into_iter().zip(values) {
            let degree_bound = labeled_commitment.degree_bound();
            let commitment = labeled_commitment.commitment();
            assert_eq!(degree_bound.is_some(), commitment.shifted_comm.is_some());

            combined_comm += &commitment.comm.0.mul(challenge_i).into();
            combined_value += &(value * challenge_i);

            if let Some(degree_bound) = degree_bound {
                let challenge_i_1 = challenge_i * opening_challenge;
                let shifted_comm = commitment.shifted_comm.as_ref().unwrap().0.into_projective();

                let shift_power = vk
                    .get_shift_power(degree_bound)
                    .ok_or(Error::UnsupportedDegreeBound(degree_bound))?;
                let mut adjusted_comm = shifted_comm - shift_power.mul(value).into_projective();
                adjusted_comm.mul_assign(challenge_i_1);
                combined_comm += &adjusted_comm;
            }
            challenge_i *= &opening_challenge.square();
        }

        end_timer!(acc_time);
        Ok((combined_comm, combined_value))
    }

    /// Accumulate `commitments` and `values` according to `opening_challenge`.
    fn accumulate_commitments_and_values_individual_opening_challenges<'a>(
        vk: &VerifierKey<E>,
        commitments: impl IntoIterator<Item = &'a LabeledCommitment<Commitment<E>>>,
        values: impl IntoIterator<Item = E::Fr>,
        opening_challenges: &dyn Fn(u64) -> E::Fr,
    ) -> Result<(E::G1Projective, E::Fr), Error> {
        let acc_time = start_timer!(|| "Accumulating commitments and values");
        let mut combined_comm = E::G1Projective::zero();
        let mut combined_value = E::Fr::zero();
        let mut opening_challenge_counter = 0;

        for (labeled_commitment, value) in commitments.into_iter().zip(values) {
            let degree_bound = labeled_commitment.degree_bound();
            let commitment = labeled_commitment.commitment();
            assert_eq!(degree_bound.is_some(), commitment.shifted_comm.is_some());

            let challenge_i = opening_challenges(opening_challenge_counter);
            opening_challenge_counter += 1;

            combined_comm += &commitment.comm.0.mul(challenge_i).into();
            combined_value += &(value * challenge_i);

            if let Some(degree_bound) = degree_bound {
                let challenge_i_1 = opening_challenges(opening_challenge_counter);
                opening_challenge_counter += 1;

                let shifted_comm = commitment.shifted_comm.as_ref().unwrap().0.into_projective();

                let shift_power = vk
                    .get_shift_power(degree_bound)
                    .ok_or(Error::UnsupportedDegreeBound(degree_bound))?;

                let mut adjusted_comm = shifted_comm - shift_power.mul(value).into_projective();

                adjusted_comm.mul_assign(challenge_i_1);
                combined_comm += &adjusted_comm;
            }
        }

        end_timer!(acc_time);
        Ok((combined_comm, combined_value))
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_camel_case_types)]

    use super::MarlinKZG10;
    use snarkvm_curves::bls12_377::Bls12_377;

    type PC<E> = MarlinKZG10<E>;
    type PC_Bls12_377 = PC<Bls12_377>;

    #[test]
    fn single_poly_test() {
        use crate::tests::*;
        single_poly_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
    }

    #[test]
    fn quadratic_poly_degree_bound_multiple_queries_test() {
        use crate::tests::*;
        quadratic_poly_degree_bound_multiple_queries_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
    }

    #[test]
    fn linear_poly_degree_bound_test() {
        use crate::tests::*;
        linear_poly_degree_bound_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
    }

    #[test]
    fn single_poly_degree_bound_test() {
        use crate::tests::*;
        single_poly_degree_bound_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
    }

    #[test]
    fn single_poly_degree_bound_multiple_queries_test() {
        use crate::tests::*;
        single_poly_degree_bound_multiple_queries_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
    }

    #[test]
    fn two_polys_degree_bound_single_query_test() {
        use crate::tests::*;
        two_polys_degree_bound_single_query_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
    }

    #[test]
    fn full_end_to_end_test() {
        use crate::tests::*;
        full_end_to_end_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
        println!("Finished bls12-377");
    }

    #[test]
    fn single_equation_test() {
        use crate::tests::*;
        single_equation_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
        println!("Finished bls12-377");
    }

    #[test]
    fn two_equation_test() {
        use crate::tests::*;
        two_equation_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
        println!("Finished bls12-377");
    }

    #[test]
    fn two_equation_degree_bound_test() {
        use crate::tests::*;
        two_equation_degree_bound_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
        println!("Finished bls12-377");
    }

    #[test]
    fn full_end_to_end_equation_test() {
        use crate::tests::*;
        full_end_to_end_equation_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
        println!("Finished bls12-377");
    }

    #[test]
    #[should_panic]
    fn bad_degree_bound_test() {
        use crate::tests::*;
        bad_degree_bound_test::<_, _, PC_Bls12_377>().expect("test failed for bls12-377");
        println!("Finished bls12-377");
    }
}
