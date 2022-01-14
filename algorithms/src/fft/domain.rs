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

//! This module contains an `EvaluationDomain` abstraction for
//! performing various kinds of polynomial arithmetic on top of
//! the scalar field.
//!
//! In pairing-based SNARKs like GM17, we need to calculate
//! a quotient polynomial over a target polynomial with roots
//! at distinct points associated with each constraint of the
//! constraint system. In order to be efficient, we choose these
//! roots to be the powers of a 2^n root of unity in the field.
//! This allows us to perform polynomial operations in O(n)
//! by performing an O(n log n) FFT over such a domain.

use crate::fft::{DomainCoeff, SparsePolynomial};
use snarkvm_fields::{batch_inversion, FftField, FftParameters};
use snarkvm_utilities::{errors::SerializationError, serialize::*};

use rand::Rng;
use std::fmt;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

/// Defines the minimum size at which to parallelize.
/// This is used as the base case in the recursive root of unity method.
#[cfg(feature = "parallel")]
const LOG_ROOTS_OF_UNITY_PARALLEL_SIZE: usize = 7;

/// Returns the log2 value of the given number.
#[cfg(feature = "parallel")]
fn log2(number: usize) -> usize {
    (number as f64).log2() as usize
}

/// Defines a domain over which finite field (I)FFTs can be performed. Works
/// only for fields that have a large multiplicative subgroup of size that is
/// a power-of-2.
#[derive(Copy, Clone, Hash, Eq, PartialEq, CanonicalSerialize, CanonicalDeserialize)]
pub struct EvaluationDomain<F: FftField> {
    /// The size of the domain.
    pub size: u64,
    /// `log_2(self.size)`.
    pub log_size_of_group: u32,
    /// Size of the domain as a field element.
    pub size_as_field_element: F,
    /// Inverse of the size in the field.
    pub size_inv: F,
    /// A generator of the subgroup.
    pub group_gen: F,
    /// Inverse of the generator of the subgroup.
    pub group_gen_inv: F,
    /// Multiplicative generator of the finite field.
    pub generator_inv: F,
}

impl<F: FftField> fmt::Debug for EvaluationDomain<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Multiplicative subgroup of size {}", self.size)
    }
}

impl<F: FftField> EvaluationDomain<F> {
    /// Sample an element that is *not* in the domain.
    pub fn sample_element_outside_domain<R: Rng>(&self, rng: &mut R) -> F {
        let mut t = F::rand(rng);
        while self.evaluate_vanishing_polynomial(t).is_zero() {
            t = F::rand(rng);
        }
        t
    }

    /// Construct a domain that is large enough for evaluations of a polynomial
    /// having `num_coeffs` coefficients.
    pub fn new(num_coeffs: usize) -> Option<Self> {
        // Compute the size of our evaluation domain
        let size = num_coeffs.next_power_of_two() as u64;
        let log_size_of_group = size.trailing_zeros();

        // libfqfft uses > https://github.com/scipr-lab/libfqfft/blob/e0183b2cef7d4c5deb21a6eaf3fe3b586d738fe0/libfqfft/evaluation_domain/domains/basic_radix2_domain.tcc#L33
        if log_size_of_group > F::FftParameters::TWO_ADICITY {
            return None;
        }

        // Compute the generator for the multiplicative subgroup.
        // It should be the 2^(log_size_of_group) root of unity.
        let group_gen = F::get_root_of_unity(size as usize)?;

        // Check that it is indeed the 2^(log_size_of_group) root of unity.
        debug_assert_eq!(group_gen.pow([size]), F::one());

        let size_as_field_element = F::from(size);
        let size_inv = size_as_field_element.inverse()?;

        Some(EvaluationDomain {
            size,
            log_size_of_group,
            size_as_field_element,
            size_inv,
            group_gen,
            group_gen_inv: group_gen.inverse()?,
            generator_inv: F::multiplicative_generator().inverse()?,
        })
    }

    /// Return the size of a domain that is large enough for evaluations of a polynomial
    /// having `num_coeffs` coefficients.
    pub fn compute_size_of_domain(num_coeffs: usize) -> Option<usize> {
        let size = num_coeffs.next_power_of_two();
        if size.trailing_zeros() <= F::FftParameters::TWO_ADICITY {
            Some(size)
        } else {
            None
        }
    }

    /// Return the size of `self`.
    pub fn size(&self) -> usize {
        self.size as usize
    }

    /// Compute an FFT.
    pub fn fft<T: DomainCoeff<F>>(&self, coeffs: &[T]) -> Vec<T> {
        let mut coeffs = coeffs.to_vec();
        self.fft_in_place(&mut coeffs);
        coeffs
    }

    /// Compute an FFT, modifying the vector in place.
    pub fn fft_in_place<T: DomainCoeff<F>>(&self, coeffs: &mut Vec<T>) {
        coeffs.resize(self.size(), T::zero());
        best_fft(coeffs, self.group_gen, self.log_size_of_group)
    }

    /// Compute an IFFT.
    pub fn ifft<T: DomainCoeff<F>>(&self, evals: &[T]) -> Vec<T> {
        let mut evals = evals.to_vec();
        self.ifft_in_place(&mut evals);
        evals
    }

    /// Compute an IFFT, modifying the vector in place.
    #[inline]
    pub fn ifft_in_place<T: DomainCoeff<F>>(&self, evals: &mut Vec<T>) {
        evals.resize(self.size(), T::zero());
        best_fft(evals, self.group_gen_inv, self.log_size_of_group);
        cfg_iter_mut!(evals).for_each(|val| *val *= self.size_inv);
    }

    /// Compute an FFT over a coset of the domain.
    pub fn coset_fft<T: DomainCoeff<F>>(&self, coeffs: &[T]) -> Vec<T> {
        let mut coeffs = coeffs.to_vec();
        self.coset_fft_in_place(&mut coeffs);
        coeffs
    }

    /// Compute an FFT over a coset of the domain, modifying the input vector
    /// in place.
    pub fn coset_fft_in_place<T: DomainCoeff<F>>(&self, coeffs: &mut Vec<T>) {
        Self::distribute_powers(coeffs, F::multiplicative_generator());
        self.fft_in_place(coeffs);
    }

    /// Compute an IFFT over a coset of the domain.
    pub fn coset_ifft<T: DomainCoeff<F>>(&self, evals: &[T]) -> Vec<T> {
        let mut evals = evals.to_vec();
        self.coset_ifft_in_place(&mut evals);
        evals
    }

    /// Compute an IFFT over a coset of the domain, modifying the input vector in place.
    pub fn coset_ifft_in_place<T: DomainCoeff<F>>(&self, evals: &mut Vec<T>) {
        self.ifft_in_place(evals);
        Self::distribute_powers(evals, self.generator_inv);
    }

    fn distribute_powers<T: DomainCoeff<F>>(coeffs: &mut Vec<T>, g: F) {
        let mut pow = F::one();
        coeffs.iter_mut().for_each(|c| {
            *c *= pow;
            pow *= &g
        })
    }

    /// Evaluate all the lagrange polynomials defined by this domain at the point
    /// `tau`.
    pub fn evaluate_all_lagrange_coefficients(&self, tau: F) -> Vec<F> {
        // Evaluate all Lagrange polynomials
        let size = self.size as usize;
        let t_size = tau.pow(&[self.size]);
        let one = F::one();
        if t_size.is_one() {
            let mut u = vec![F::zero(); size];
            let mut omega_i = one;
            for x in u.iter_mut().take(size) {
                if omega_i == tau {
                    *x = one;
                    break;
                }
                omega_i *= &self.group_gen;
            }
            u
        } else {
            let mut l = (t_size - one) * self.size_inv;
            let mut r = one;
            let mut u = vec![F::zero(); size];
            let mut ls = vec![F::zero(); size];
            for i in 0..size {
                u[i] = tau - r;
                ls[i] = l;
                l *= &self.group_gen;
                r *= &self.group_gen;
            }

            batch_inversion(u.as_mut_slice());
            cfg_iter_mut!(u).zip(ls).for_each(|(tau_minus_r, l)| {
                *tau_minus_r = l * *tau_minus_r;
            });
            u
        }
    }

    /// Return the sparse vanishing polynomial.
    pub fn vanishing_polynomial(&self) -> SparsePolynomial<F> {
        let coeffs = vec![(0, -F::one()), (self.size(), F::one())];
        SparsePolynomial::from_coefficients_vec(coeffs)
    }

    /// This evaluates the vanishing polynomial for this domain at tau.
    /// For multiplicative subgroups, this polynomial is `z(X) = X^self.size - 1`.
    pub fn evaluate_vanishing_polynomial(&self, tau: F) -> F {
        tau.pow(&[self.size]) - F::one()
    }

    /// Return an iterator over the elements of the domain.
    pub fn elements(&self) -> Elements<F> {
        Elements {
            cur_elem: F::one(),
            cur_pow: 0,
            domain: *self,
        }
    }

    /// The target polynomial is the zero polynomial in our
    /// evaluation domain, so we must perform division over
    /// a coset.
    pub fn divide_by_vanishing_poly_on_coset_in_place(&self, evals: &mut [F]) {
        let i = self
            .evaluate_vanishing_polynomial(F::multiplicative_generator())
            .inverse()
            .unwrap();

        cfg_iter_mut!(evals).for_each(|eval| *eval *= &i);
    }

    /// Given an index which assumes the first elements of this domain are the elements of
    /// another (sub)domain with size size_s,
    /// this returns the actual index into this domain.
    pub fn reindex_by_subdomain(&self, other: Self, index: usize) -> usize {
        assert!(self.size() >= other.size());
        // Let this subgroup be G, and the subgroup we're re-indexing by be S.
        // Since its a subgroup, the 0th element of S is at index 0 in G, the first element of S is at
        // index |G|/|S|, the second at 2*|G|/|S|, etc.
        // Thus for an index i that corresponds to S, the index in G is i*|G|/|S|
        let period = self.size() / other.size();
        if index < other.size() {
            index * period
        } else {
            // Let i now be the index of this element in G \ S
            // Let x be the number of elements in G \ S, for every element in S. Then x = (|G|/|S| - 1).
            // At index i in G \ S, the number of elements in S that appear before the index in G to which
            // i corresponds to, is floor(i / x) + 1.
            // The +1 is because index 0 of G is S_0, so the position is offset by at least one.
            // The floor(i / x) term is because after x elements in G \ S, there is one more element from S
            // that will have appeared in G.
            let i = index - other.size();
            let x = period - 1;
            i + (i / x) + 1
        }
    }

    /// Perform O(n) multiplication of two polynomials that are presented by their
    /// evaluations in the domain.
    /// Returns the evaluations of the product over the domain.
    #[must_use]
    pub fn mul_polynomials_in_evaluation_domain(&self, self_evals: &[F], other_evals: &[F]) -> Vec<F> {
        assert_eq!(self_evals.len(), other_evals.len());

        let mut result = self_evals.to_vec();
        cfg_iter_mut!(result).zip(other_evals).for_each(|(a, b)| *a *= b);

        result
    }

    /// Computes the first `self.size / 2` roots of unity for the entire domain.
    /// e.g. for the domain [1, g, g^2, ..., g^{n - 1}], it computes
    // [1, g, g^2, ..., g^{(n/2) - 1}]
    #[cfg(not(feature = "parallel"))]
    pub fn roots_of_unity(&self, root: F) -> Vec<F> {
        Self::compute_powers_serial((self.size as usize) / 2, root)
    }

    /// Computes the first `self.size / 2` roots of unity.
    #[cfg(feature = "parallel")]
    pub fn roots_of_unity(&self, root: F) -> Vec<F> {
        // TODO: check if this method can replace parallel compute powers.
        let log_size = log2(self.size as usize);

        // Early exit for short inputs.
        if log_size <= LOG_ROOTS_OF_UNITY_PARALLEL_SIZE {
            Self::compute_powers_serial((self.size as usize) / 2, root)
        } else {
            let mut tmp = root;
            // w, w^2, w^4, w^8, ..., w^(2^(log_size - 1))
            let log_powers: Vec<F> = (0..(log_size - 1))
                .map(|_| {
                    let old_value = tmp;
                    tmp.square_in_place();
                    old_value
                })
                .collect();

            // Allocate the return array and start the recursion.
            let mut powers = vec![F::zero(); 1 << (log_size - 1)];
            Self::roots_of_unity_recursive(&mut powers, &log_powers);
            powers
        }
    }

    #[cfg(feature = "parallel")]
    fn roots_of_unity_recursive(powers: &mut [F], log_powers: &[F]) {
        assert_eq!(powers.len(), 1 << log_powers.len());

        // Base case: compute the powers sequentially,
        // g = log_powers[0], out = [1, g, g^2, ...]
        if log_powers.len() <= LOG_ROOTS_OF_UNITY_PARALLEL_SIZE {
            powers[0] = F::one();
            for i in 1..powers.len() {
                powers[i] = powers[i - 1] * log_powers[0];
            }
            return;
        }

        // Recursive case:
        // 1. Split log_powers in half.
        let (lr_low, lr_high) = log_powers.split_at((1 + log_powers.len()) / 2);
        let mut scr_low = vec![F::default(); 1 << lr_low.len()];
        let mut scr_high = vec![F::default(); 1 << lr_high.len()];

        // 2. Compute each half individually
        rayon::join(
            || Self::roots_of_unity_recursive(&mut scr_low, lr_low),
            || Self::roots_of_unity_recursive(&mut scr_high, lr_high),
        );

        // 3. Recombine the halves.
        // At this point, out is a blank slice.
        powers
            .par_chunks_mut(scr_low.len())
            .zip(&scr_high)
            .for_each(|(power_chunk, scr_high)| {
                for (power, scr_low) in power_chunk.iter_mut().zip(&scr_low) {
                    *power = *scr_high * scr_low;
                }
            });
    }

    fn compute_powers_serial(size: usize, root: F) -> Vec<F> {
        let mut value = F::one();
        (0..size)
            .map(|_| {
                let old_value = value;
                value *= &root;
                old_value
            })
            .collect()
    }
}

#[allow(unused_variables)]
#[cfg(not(feature = "parallel"))]
fn best_fft<T: DomainCoeff<F>, F: FftField>(a: &mut [T], omega: F, log_n: u32) {
    serial_radix2_fft(a, omega, log_n);
}

lazy_static::lazy_static! {
    static ref LOG_CPUS: u32 = log2_floor(rayon::current_num_threads());
}

#[cfg(feature = "parallel")]
fn best_fft<T: DomainCoeff<F>, F: FftField>(a: &mut [T], omega: F, log_n: u32) {
    // let num_cpus = rayon::current_num_threads();
    // let log_cpus = log2_floor(num_cpus);

    if log_n <= *LOG_CPUS {
        serial_radix2_fft::<T, F>(a, omega, log_n);
    } else {
        parallel_radix2_fft::<T, F>(a, omega, log_n, *LOG_CPUS);
    }
}

#[allow(clippy::many_single_char_names)]
pub(crate) fn serial_radix2_fft<T: DomainCoeff<F>, F: FftField>(a: &mut [T], omega: F, log_n: u32) {
    #[inline]
    fn bitreverse(mut n: u32, l: u32) -> u32 {
        let mut r = 0;
        for _ in 0..l {
            r = (r << 1) | (n & 1);
            n >>= 1;
        }
        r
    }

    let n = a.len() as u32;
    assert_eq!(n, 1 << log_n);

    for k in 0..n {
        let rk = bitreverse(k, log_n);
        if k < rk {
            a.swap(rk as usize, k as usize);
        }
    }

    let mut m = 1;
    for _ in 0..log_n {
        let w_m = omega.pow(&[(n / (2 * m)) as u64]);

        let mut k = 0;
        while k < n {
            let mut w = F::one();
            for j in 0..m {
                let mut t = a[(k + j + m) as usize];
                t *= w;
                let mut tmp = a[(k + j) as usize];
                tmp -= t;
                a[(k + j + m) as usize] = tmp;
                a[(k + j) as usize] += t;
                w.mul_assign(&w_m);
            }

            k += 2 * m;
        }

        m *= 2;
    }
}

#[cfg(feature = "parallel")]
pub(crate) fn parallel_radix2_fft<T: DomainCoeff<F>, F: FftField>(a: &mut [T], omega: F, log_n: u32, log_cpus: u32) {
    assert!(log_n >= log_cpus);

    let m = a.len();
    let num_chunks = 1 << (log_cpus as usize);
    assert_eq!(m % num_chunks, 0);
    let m_div_num_chunks = m / num_chunks;

    let mut tmp = vec![vec![T::zero(); m_div_num_chunks]; num_chunks];
    let new_omega = omega.pow(&[num_chunks as u64]);
    let new_two_adicity = F::k_adicity(2, m_div_num_chunks);

    tmp.par_iter_mut().enumerate().for_each(|(j, tmp)| {
        // Shuffle into a sub-FFT
        let omega_j = omega.pow(&[j as u64]);
        let omega_step = omega.pow(&[(j * m_div_num_chunks) as u64]);

        let mut elt = F::one();
        for (i, tmp_t) in tmp.iter_mut().enumerate().take(m_div_num_chunks) {
            for s in 0..num_chunks {
                let idx = (i + (s * m_div_num_chunks)) % m;
                let mut t = a[idx];
                t *= elt;
                *tmp_t += t;
                elt *= &omega_step;
            }
            elt *= &omega_j;
        }

        // Perform sub-FFT
        serial_radix2_fft(tmp, new_omega, new_two_adicity);
    });

    a.iter_mut()
        .enumerate()
        .for_each(|(i, a)| *a = tmp[i % num_chunks][i / num_chunks]);
}

/// An iterator over the elements of the domain.
pub struct Elements<F: FftField> {
    cur_elem: F,
    cur_pow: u64,
    domain: EvaluationDomain<F>,
}

impl<F: FftField> Iterator for Elements<F> {
    type Item = F;

    fn next(&mut self) -> Option<F> {
        if self.cur_pow == self.domain.size {
            None
        } else {
            let cur_elem = self.cur_elem;
            self.cur_elem *= &self.domain.group_gen;
            self.cur_pow += 1;
            Some(cur_elem)
        }
    }
}

pub(crate) fn log2_floor(num: usize) -> u32 {
    assert!(num > 0);
    let mut pow = 0;
    while (1 << (pow + 1)) <= num {
        pow += 1;
    }
    pow
}

#[test]
fn test_log2_floor() {
    assert_eq!(log2_floor(1), 0);
    assert_eq!(log2_floor(2), 1);
    assert_eq!(log2_floor(3), 1);
    assert_eq!(log2_floor(4), 2);
    assert_eq!(log2_floor(5), 2);
    assert_eq!(log2_floor(6), 2);
    assert_eq!(log2_floor(7), 2);
    assert_eq!(log2_floor(8), 3);
}

#[cfg(test)]
mod tests {
    use crate::fft::{DensePolynomial, EvaluationDomain};
    use snarkvm_curves::bls12_377::Fr;
    use snarkvm_fields::{FftField, Field, One, Zero};
    use snarkvm_utilities::UniformRand;

    use rand::{thread_rng, Rng};

    #[test]
    fn vanishing_polynomial_evaluation() {
        let rng = &mut thread_rng();
        for coeffs in 0..10 {
            let domain = EvaluationDomain::<Fr>::new(coeffs).unwrap();
            let z = domain.vanishing_polynomial();
            for _ in 0..100 {
                let point = rng.gen();
                assert_eq!(z.evaluate(point), domain.evaluate_vanishing_polynomial(point))
            }
        }
    }

    #[test]
    fn vanishing_polynomial_vanishes_on_domain() {
        for coeffs in 0..1000 {
            let domain = EvaluationDomain::<Fr>::new(coeffs).unwrap();
            let z = domain.vanishing_polynomial();
            for point in domain.elements() {
                assert!(z.evaluate(point).is_zero())
            }
        }
    }

    #[test]
    fn size_of_elements() {
        for coeffs in 1..10 {
            let size = 1 << coeffs;
            let domain = EvaluationDomain::<Fr>::new(size).unwrap();
            let domain_size = domain.size();
            assert_eq!(domain_size, domain.elements().count());
        }
    }

    #[test]
    fn elements_contents() {
        for coeffs in 1..10 {
            let size = 1 << coeffs;
            let domain = EvaluationDomain::<Fr>::new(size).unwrap();
            for (i, element) in domain.elements().enumerate() {
                assert_eq!(element, domain.group_gen.pow([i as u64]));
            }
        }
    }

    /// Test that lagrange interpolation for a random polynomial at a random point works.
    #[test]
    fn non_systematic_lagrange_coefficients_test() {
        for domain_dimension in 1..10 {
            let domain_size = 1 << domain_dimension;
            let domain = EvaluationDomain::<Fr>::new(domain_size).unwrap();
            // Get random point & lagrange coefficients
            let random_point = Fr::rand(&mut thread_rng());
            let lagrange_coefficients = domain.evaluate_all_lagrange_coefficients(random_point);

            // Sample the random polynomial, evaluate it over the domain and the random point.
            let random_polynomial = DensePolynomial::<Fr>::rand(domain_size - 1, &mut thread_rng());
            let polynomial_evaluations = domain.fft(random_polynomial.coeffs());
            let actual_evaluations = random_polynomial.evaluate(random_point);

            // Do lagrange interpolation, and compare against the actual evaluation
            let mut interpolated_evaluation = Fr::zero();
            for i in 0..domain_size {
                interpolated_evaluation += lagrange_coefficients[i] * polynomial_evaluations[i];
            }
            assert_eq!(actual_evaluations, interpolated_evaluation);
        }
    }

    /// Test that lagrange coefficients for a point in the domain is correct.
    #[test]
    fn systematic_lagrange_coefficients_test() {
        // This runs in time O(N^2) in the domain size, so keep the domain dimension low.
        // We generate lagrange coefficients for each element in the domain.
        for domain_dimension in 1..5 {
            let domain_size = 1 << domain_dimension;
            let domain = EvaluationDomain::<Fr>::new(domain_size).unwrap();
            let all_domain_elements: Vec<Fr> = domain.elements().collect();
            for (i, domain_element) in all_domain_elements.iter().enumerate().take(domain_size) {
                let lagrange_coefficients = domain.evaluate_all_lagrange_coefficients(*domain_element);
                for (j, lagrange_coefficient) in lagrange_coefficients.iter().enumerate().take(domain_size) {
                    // Lagrange coefficient for the evaluation point, which should be 1
                    if i == j {
                        assert_eq!(*lagrange_coefficient, Fr::one());
                    } else {
                        assert_eq!(*lagrange_coefficient, Fr::zero());
                    }
                }
            }
        }
    }

    /// Tests that the roots of unity result is the same as domain.elements().
    #[test]
    fn test_roots_of_unity() {
        let max_degree = 10;
        for log_domain_size in 0..max_degree {
            let domain_size = 1 << log_domain_size;
            let domain = EvaluationDomain::<Fr>::new(domain_size).unwrap();
            let actual_roots = domain.roots_of_unity(domain.group_gen);
            for &value in &actual_roots {
                assert!(domain.evaluate_vanishing_polynomial(value).is_zero());
            }
            let expected_roots_elements = domain.elements();
            for (expected, &actual) in expected_roots_elements.zip(&actual_roots) {
                assert_eq!(expected, actual);
            }
            assert_eq!(actual_roots.len(), domain_size / 2);
        }
    }

    /// Tests that the FFTs output the correct result.
    #[test]
    fn test_fft_correctness() {
        // This assumes a correct polynomial evaluation at point procedure.
        // It tests consistency of FFT/IFFT, and coset_fft/coset_ifft,
        // along with testing that each individual evaluation is correct.

        // Runs in time O(degree^2)
        let log_degree = 5;
        let degree = 1 << log_degree;
        let random_polynomial = DensePolynomial::<Fr>::rand(degree - 1, &mut thread_rng());

        for log_domain_size in log_degree..(log_degree + 2) {
            let domain_size = 1 << log_domain_size;
            let domain = EvaluationDomain::<Fr>::new(domain_size).unwrap();
            let polynomial_evaluations = domain.fft(&random_polynomial.coeffs);
            let polynomial_coset_evaluations = domain.coset_fft(&random_polynomial.coeffs);
            for (i, x) in domain.elements().enumerate() {
                let coset_x = Fr::multiplicative_generator() * x;

                assert_eq!(polynomial_evaluations[i], random_polynomial.evaluate(x));
                assert_eq!(polynomial_coset_evaluations[i], random_polynomial.evaluate(coset_x));
            }

            let randon_polynomial_from_subgroup =
                DensePolynomial::from_coefficients_vec(domain.ifft(&polynomial_evaluations));
            let random_polynomial_from_coset =
                DensePolynomial::from_coefficients_vec(domain.coset_ifft(&polynomial_coset_evaluations));

            assert_eq!(
                random_polynomial, randon_polynomial_from_subgroup,
                "degree = {}, domain size = {}",
                degree, domain_size
            );
            assert_eq!(
                random_polynomial, random_polynomial_from_coset,
                "degree = {}, domain size = {}",
                degree, domain_size
            );
        }
    }
}
