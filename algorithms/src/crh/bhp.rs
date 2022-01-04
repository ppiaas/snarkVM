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

use crate::{hash_to_curve::hash_to_curve, CRHError, CRH};
use snarkvm_curves::{AffineCurve, ProjectiveCurve};
use snarkvm_fields::{ConstraintFieldError, Field, PrimeField, ToConstraintField};
use snarkvm_utilities::{BigInteger, FromBytes, ToBytes};

use once_cell::sync::OnceCell;
use std::{
    borrow::Borrow,
    fmt::Debug,
    io::{Read, Result as IoResult, Write},
    sync::Arc,
};

#[cfg(feature = "parallel")]
use rayon::prelude::*;

// The stack is currently allocated with the following size
// because we cannot specify them using the trait consts.
const MAX_WINDOW_SIZE: usize = 256;
const MAX_NUM_WINDOWS: usize = 2048;

pub const BOWE_HOPWOOD_CHUNK_SIZE: usize = 3;
pub const BOWE_HOPWOOD_LOOKUP_SIZE: usize = 2usize.pow(BOWE_HOPWOOD_CHUNK_SIZE as u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BHPCRH<G: ProjectiveCurve, const NUM_WINDOWS: usize, const WINDOW_SIZE: usize> {
    pub bases: Arc<Vec<Vec<G>>>,
    base_lookup: OnceCell<Vec<Vec<[G; BOWE_HOPWOOD_LOOKUP_SIZE]>>>,
}

impl<G: ProjectiveCurve, const NUM_WINDOWS: usize, const WINDOW_SIZE: usize> CRH
    for BHPCRH<G, NUM_WINDOWS, WINDOW_SIZE>
{
    type Output = <G::Affine as AffineCurve>::BaseField;
    type Parameters = Arc<Vec<Vec<G>>>;

    fn setup(message: &str) -> Self {
        fn calculate_num_chunks_in_segment<F: PrimeField>() -> usize {
            let upper_limit = F::modulus_minus_one_div_two();
            let mut c = 0;
            let mut range = F::BigInteger::from(2_u64);
            while range < upper_limit {
                range.muln(4);
                c += 1;
            }

            c
        }

        let maximum_num_chunks_in_segment = calculate_num_chunks_in_segment::<G::ScalarField>();
        if WINDOW_SIZE > maximum_num_chunks_in_segment {
            panic!(
                "BHP CRH must have a window size resulting in scalars < (p-1)/2, \
                 maximum segment size is {}",
                maximum_num_chunks_in_segment
            );
        }

        let time = start_timer!(|| format!(
            "BoweHopwoodPedersenCRH::Setup: {} segments of {} 3-bit chunks; {{0,1}}^{{{}}} -> G",
            NUM_WINDOWS,
            WINDOW_SIZE,
            WINDOW_SIZE * NUM_WINDOWS * BOWE_HOPWOOD_CHUNK_SIZE
        ));
        let bases = Arc::new(Self::create_generators(message));
        end_timer!(time);

        Self {
            bases,
            base_lookup: OnceCell::new(),
        }
    }

    fn hash_bits(&self, input: &[bool]) -> Result<Self::Output, CRHError> {
        let affine = self.hash_bits_inner(input.iter(), input.len())?.into_affine();
        debug_assert!(affine.is_in_correct_subgroup_assuming_on_curve());
        Ok(affine.to_x_coordinate())
    }

    fn parameters(&self) -> &Self::Parameters {
        &self.bases
    }
}

impl<G: ProjectiveCurve, const NUM_WINDOWS: usize, const WINDOW_SIZE: usize> BHPCRH<G, NUM_WINDOWS, WINDOW_SIZE> {
    pub fn create_generators(message: &str) -> Vec<Vec<G>> {
        let mut generators = Vec::with_capacity(NUM_WINDOWS);
        for index in 0..NUM_WINDOWS {
            // Construct an indexed message to attempt to sample a base.
            let indexed_message = format!("{} at {}", message, index);
            let (generator, _, _) = hash_to_curve::<G::Affine>(&indexed_message);
            let mut base = generator.into_projective();
            // Compute the generators for the sampled base.
            let mut generators_for_segment = Vec::with_capacity(WINDOW_SIZE);
            for _ in 0..WINDOW_SIZE {
                generators_for_segment.push(base);
                for _ in 0..4 {
                    base.double_in_place();
                }
            }
            generators.push(generators_for_segment);
        }
        generators
    }

    pub fn base_lookup(&self, bases: &[Vec<G>]) -> &Vec<Vec<[G; BOWE_HOPWOOD_LOOKUP_SIZE]>> {
        self.base_lookup
            .get_or_try_init::<_, ()>(|| {
                Ok(cfg_iter!(bases)
                    .map(|x| {
                        x.iter()
                            .map(|g| {
                                let mut out = [G::zero(); BOWE_HOPWOOD_LOOKUP_SIZE];
                                for (i, element) in out.iter_mut().enumerate().take(BOWE_HOPWOOD_LOOKUP_SIZE) {
                                    let mut encoded = *g;
                                    if (i & 0x01) != 0 {
                                        encoded += g;
                                    }
                                    if (i & 0x02) != 0 {
                                        encoded += g.double();
                                    }
                                    if (i & 0x04) != 0 {
                                        encoded = encoded.neg();
                                    }
                                    *element = encoded;
                                }
                                out
                            })
                            .collect()
                    })
                    .collect())
            })
            .expect("failed to init BoweHopwoodPedersenCRHParameters")
    }

    /// Precondition: number of elements in `input` == `num_bits`.
    pub(crate) fn hash_bits_inner<S: Borrow<bool>>(
        &self,
        input: impl Iterator<Item = S>,
        num_bits: usize,
    ) -> Result<G, CRHError> {
        if num_bits > WINDOW_SIZE * NUM_WINDOWS {
            return Err(CRHError::IncorrectInputLength(num_bits, WINDOW_SIZE, NUM_WINDOWS));
        }
        debug_assert!(WINDOW_SIZE <= MAX_WINDOW_SIZE);
        debug_assert!(NUM_WINDOWS <= MAX_NUM_WINDOWS);

        // overzealous but stack allocation
        let mut buf_slice = [false; MAX_WINDOW_SIZE * MAX_NUM_WINDOWS + BOWE_HOPWOOD_CHUNK_SIZE + 1];
        buf_slice[..num_bits]
            .iter_mut()
            .zip(input)
            .for_each(|(b, i)| *b = *i.borrow());

        let mut bit_len = WINDOW_SIZE * NUM_WINDOWS;
        if bit_len % BOWE_HOPWOOD_CHUNK_SIZE != 0 {
            bit_len += BOWE_HOPWOOD_CHUNK_SIZE - (bit_len % BOWE_HOPWOOD_CHUNK_SIZE);
        }

        debug_assert_eq!(bit_len % BOWE_HOPWOOD_CHUNK_SIZE, 0);

        debug_assert_eq!(
            self.bases.len(),
            NUM_WINDOWS,
            "Incorrect number of windows ({:?}) for BHP of {:?}x{:?}x{}",
            self.bases.len(),
            WINDOW_SIZE,
            NUM_WINDOWS,
            BOWE_HOPWOOD_CHUNK_SIZE,
        );
        for bases in self.bases.iter() {
            debug_assert_eq!(bases.len(), WINDOW_SIZE);
        }
        let base_lookup = self.base_lookup(&self.bases);
        debug_assert_eq!(base_lookup.len(), NUM_WINDOWS);
        for bases in base_lookup.iter() {
            debug_assert_eq!(bases.len(), WINDOW_SIZE);
        }
        debug_assert_eq!(BOWE_HOPWOOD_CHUNK_SIZE, 3);

        // Compute sum of h_i^{sum of
        // (1-2*c_{i,j,2})*(1+c_{i,j,0}+2*c_{i,j,1})*2^{4*(j-1)} for all j in segment}
        // for all i. Described in section 5.4.1.7 in the Zcash protocol
        // specification.
        let output = buf_slice[..bit_len]
            .chunks(WINDOW_SIZE * BOWE_HOPWOOD_CHUNK_SIZE)
            .zip(base_lookup)
            .map(|(segment_bits, segment_generators)| {
                segment_bits
                    .chunks(BOWE_HOPWOOD_CHUNK_SIZE)
                    .zip(segment_generators)
                    .map(|(chunk_bits, generator)| {
                        &generator
                            [(chunk_bits[0] as usize) | (chunk_bits[1] as usize) << 1 | (chunk_bits[2] as usize) << 2]
                    })
                    .fold(G::zero(), |a, b| a + b)
            })
            .fold(G::zero(), |a, b| a + b);

        Ok(output)
    }
}

impl<G: ProjectiveCurve, const NUM_WINDOWS: usize, const WINDOW_SIZE: usize> From<Arc<Vec<Vec<G>>>>
    for BHPCRH<G, NUM_WINDOWS, WINDOW_SIZE>
{
    fn from(bases: Arc<Vec<Vec<G>>>) -> Self {
        Self {
            bases,
            base_lookup: OnceCell::new(),
        }
    }
}

impl<G: ProjectiveCurve, const NUM_WINDOWS: usize, const WINDOW_SIZE: usize> ToBytes
    for BHPCRH<G, NUM_WINDOWS, WINDOW_SIZE>
{
    fn write_le<W: Write>(&self, mut writer: W) -> IoResult<()> {
        (self.bases.len() as u32).write_le(&mut writer)?;
        for base in self.bases.iter() {
            (base.len() as u32).write_le(&mut writer)?;
            for g in base {
                g.write_le(&mut writer)?;
            }
        }
        Ok(())
    }
}

impl<G: ProjectiveCurve, const NUM_WINDOWS: usize, const WINDOW_SIZE: usize> FromBytes
    for BHPCRH<G, NUM_WINDOWS, WINDOW_SIZE>
{
    #[inline]
    fn read_le<R: Read>(mut reader: R) -> IoResult<Self> {
        let num_bases: u32 = FromBytes::read_le(&mut reader)?;
        let mut bases = Vec::with_capacity(num_bases as usize);

        for _ in 0..num_bases {
            let base_len: u32 = FromBytes::read_le(&mut reader)?;
            let mut base = Vec::with_capacity(base_len as usize);

            for _ in 0..base_len {
                let g: G = FromBytes::read_le(&mut reader)?;
                base.push(g);
            }
            bases.push(base);
        }

        Ok(Self {
            bases: Arc::new(bases),
            base_lookup: OnceCell::new(),
        })
    }
}

impl<F: Field, G: ProjectiveCurve + ToConstraintField<F>, const NUM_WINDOWS: usize, const WINDOW_SIZE: usize>
    ToConstraintField<F> for BHPCRH<G, NUM_WINDOWS, WINDOW_SIZE>
{
    #[inline]
    fn to_field_elements(&self) -> Result<Vec<F>, ConstraintFieldError> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snarkvm_curves::edwards_bls12::EdwardsProjective;

    const NUM_WINDOWS: usize = 8;
    const WINDOW_SIZE: usize = 32;

    #[test]
    fn test_bhp_sanity_check() {
        let crh = <BHPCRH<EdwardsProjective, NUM_WINDOWS, WINDOW_SIZE> as CRH>::setup("test_bowe_pedersen");
        let input = vec![127u8; 32];

        let output = crh.hash(&input).unwrap();
        assert_eq!(
            &*output.to_string(),
            "2591648422993904809826711498838675948697848925001720514073745852367402669969"
        );
    }
}
