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

use std::any::TypeId;

#[cfg(all(feature = "cuda", target_arch = "x86_64"))]
use std::sync::atomic::{AtomicBool, Ordering};

use snarkvm_curves::{bls12_377::G1Affine, traits::AffineCurve};
use snarkvm_fields::{PrimeField, Zero};
use snarkvm_utilities::BitIteratorBE;

mod standard;

#[cfg(all(feature = "cuda", target_arch = "x86_64"))]
mod cuda;

#[cfg(all(feature = "cuda", target_arch = "x86_64"))]
static HAS_CUDA_FAILED: AtomicBool = AtomicBool::new(false);

#[cfg(all(feature = "cuda", target_arch = "x86_64"))]
pub fn is_cuda_enabled() -> bool {
    std::env::var("ALEO_CUDA_ENABLED")
        .and_then(|v| match v.parse() {
            Ok(val) => Ok(val),
            Err(_) => Ok(true),
        })
        .unwrap_or(true)
}

pub struct VariableBaseMSM;

impl VariableBaseMSM {
    #[allow(unused)]
    fn msm_naive<G: AffineCurve>(bases: &[G], scalars: &[<G::ScalarField as PrimeField>::BigInteger]) -> G::Projective {
        let mut acc = G::Projective::zero();

        for (base, scalar) in bases.iter().zip(scalars.iter()) {
            acc += base.mul_bits(BitIteratorBE::new(*scalar));
        }
        acc
    }

    pub fn multi_scalar_mul<G: AffineCurve>(
        bases: &[G],
        scalars: &[<G::ScalarField as PrimeField>::BigInteger],
    ) -> G::Projective {
        if TypeId::of::<G>() == TypeId::of::<G1Affine>() {
            #[cfg(all(feature = "cuda", target_arch = "x86_64"))]
            {
                if !is_cuda_enabled() {
                    return standard::msm_standard(bases, scalars);
                }
                if !HAS_CUDA_FAILED.load(Ordering::SeqCst) {
                    match cuda::msm_cuda(bases, scalars) {
                        Ok(x) => return x,
                        Err(e) => {
                            HAS_CUDA_FAILED.store(true, Ordering::SeqCst);
                            eprintln!("CUDA failed, moving to next msm method. Error: {:?}", e);
                        }
                    }
                }
            }
        }
        standard::msm_standard(bases, scalars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_xorshift::XorShiftRng;
    use snarkvm_curves::{
        bls12_377::{Fr, G1Affine, G1Projective},
        traits::ProjectiveCurve,
    };
    use snarkvm_fields::PrimeField;
    use snarkvm_utilities::{rand::UniformRand, BigInteger256};

    fn test_data(seed: u64, samples: usize) -> (Vec<G1Affine>, Vec<BigInteger256>) {
        let mut rng = XorShiftRng::seed_from_u64(seed);

        let v = (0..samples).map(|_| Fr::rand(&mut rng).to_repr()).collect::<Vec<_>>();
        let g = (0..samples)
            .map(|_| G1Projective::rand(&mut rng).into_affine())
            .collect::<Vec<_>>();

        (g, v)
    }

    #[test]
    fn test_naive() {
        let (bases, scalars) = test_data(334563456, 100);
        let rust = standard::msm_standard(bases.as_slice(), scalars.as_slice());
        let naive = VariableBaseMSM::msm_naive(bases.as_slice(), scalars.as_slice());
        assert_eq!(rust, naive);
    }

    #[test]
    fn test_multi_scalar_mul() {
        let (bases, scalars) = test_data(334563456, 100);
        let rust = VariableBaseMSM::msm_naive(bases.as_slice(), scalars.as_slice());
        let naive = VariableBaseMSM::multi_scalar_mul(bases.as_slice(), scalars.as_slice());
        assert_eq!(rust, naive);
    }

    #[cfg(all(feature = "cuda", target_arch = "x86_64"))]
    #[test]
    fn test_msm_cuda() {
        for i in 0..100 {
            let (bases, scalars) = test_data(334563456 + i as u64, 1 << 10);
            let rust = standard::msm_standard(bases.as_slice(), scalars.as_slice());

            let cuda = cuda::msm_cuda(bases.as_slice(), scalars.as_slice()).unwrap();
            assert_eq!(rust, cuda);
        }
    }
}
