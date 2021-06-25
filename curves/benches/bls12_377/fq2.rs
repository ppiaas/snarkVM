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

use snarkvm_curves::bls12_377::Fq2;
use snarkvm_fields::{Field, SquareRootField};
use snarkvm_utilities::rand::UniformRand;

use criterion::Criterion;
use rand::SeedableRng;
use rand_xorshift::XorShiftRng;
use std::ops::{AddAssign, MulAssign, SubAssign};

pub(crate) fn bench_fq2_add_assign(c: &mut Criterion) {
    const SAMPLES: usize = 1000;

    let mut rng = XorShiftRng::seed_from_u64(1231275789u64);

    let v: Vec<(Fq2, Fq2)> = (0..SAMPLES)
        .map(|_| (Fq2::rand(&mut rng), Fq2::rand(&mut rng)))
        .collect();

    let mut count = 0;
    c.bench_function("bls12_377: fq2_add_assign", |c| {
        c.iter(|| {
            let mut tmp = v[count].0;
            tmp.add_assign(v[count].1);
            count = (count + 1) % SAMPLES;
            tmp
        })
    });
}

pub(crate) fn bench_fq2_sub_assign(c: &mut Criterion) {
    const SAMPLES: usize = 1000;

    let mut rng = XorShiftRng::seed_from_u64(1231275789u64);

    let v: Vec<(Fq2, Fq2)> = (0..SAMPLES)
        .map(|_| (Fq2::rand(&mut rng), Fq2::rand(&mut rng)))
        .collect();

    let mut count = 0;
    c.bench_function("bls12_377: fq2_sub_assign", |c| {
        c.iter(|| {
            let mut tmp = v[count].0;
            tmp.sub_assign(&v[count].1);
            count = (count + 1) % SAMPLES;
            tmp
        })
    });
}

pub(crate) fn bench_fq2_mul_assign(c: &mut Criterion) {
    const SAMPLES: usize = 1000;

    let mut rng = XorShiftRng::seed_from_u64(1231275789u64);

    let v: Vec<(Fq2, Fq2)> = (0..SAMPLES)
        .map(|_| (Fq2::rand(&mut rng), Fq2::rand(&mut rng)))
        .collect();

    let mut count = 0;
    c.bench_function("bls12_377: fq2_mul_assign", |c| {
        c.iter(|| {
            let mut tmp = v[count].0;
            tmp.mul_assign(&v[count].1);
            count = (count + 1) % SAMPLES;
            tmp
        })
    });
}

pub(crate) fn bench_fq2_double(c: &mut Criterion) {
    const SAMPLES: usize = 1000;

    let mut rng = XorShiftRng::seed_from_u64(1231275789u64);

    let v: Vec<Fq2> = (0..SAMPLES).map(|_| Fq2::rand(&mut rng)).collect();

    let mut count = 0;
    c.bench_function("bls12_377: fq2_double", |c| {
        c.iter(|| {
            let mut tmp = v[count];
            tmp.double_in_place();
            count = (count + 1) % SAMPLES;
            tmp
        })
    });
}

pub(crate) fn bench_fq2_square(c: &mut Criterion) {
    const SAMPLES: usize = 1000;

    let mut rng = XorShiftRng::seed_from_u64(1231275789u64);

    let v: Vec<Fq2> = (0..SAMPLES).map(|_| Fq2::rand(&mut rng)).collect();

    let mut count = 0;
    c.bench_function("bls12_377: fq2_square", |c| {
        c.iter(|| {
            let mut tmp = v[count];
            tmp.square_in_place();
            count = (count + 1) % SAMPLES;
            tmp
        })
    });
}

pub(crate) fn bench_fq2_inverse(c: &mut Criterion) {
    const SAMPLES: usize = 1000;

    let mut rng = XorShiftRng::seed_from_u64(1231275789u64);

    let v: Vec<Fq2> = (0..SAMPLES).map(|_| Fq2::rand(&mut rng)).collect();

    let mut count = 0;
    c.bench_function("bls12_377: fq2_inverse", |c| {
        c.iter(|| {
            let tmp = v[count].inverse();
            count = (count + 1) % SAMPLES;
            tmp
        })
    });
}

pub(crate) fn bench_fq2_sqrt(c: &mut Criterion) {
    const SAMPLES: usize = 1000;

    let mut rng = XorShiftRng::seed_from_u64(1231275789u64);

    let v: Vec<Fq2> = (0..SAMPLES).map(|_| Fq2::rand(&mut rng)).collect();

    let mut count = 0;
    c.bench_function("bls12_377: fq2_sqrt", |c| {
        c.iter(|| {
            let tmp = v[count].sqrt();
            count = (count + 1) % SAMPLES;
            tmp
        })
    });
}
