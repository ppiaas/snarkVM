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

#![allow(clippy::module_inception)]
#![allow(clippy::type_complexity)]

#[macro_use]
extern crate derivative;

#[macro_use]
extern crate snarkvm_profiler;

#[macro_use]
extern crate thiserror;

#[macro_use]
pub mod macros;

#[cfg(feature = "commitment")]
pub mod commitment;

#[cfg(feature = "crh")]
pub mod crh;

#[cfg(feature = "crypto_hash")]
pub mod crypto_hash;

#[cfg(feature = "encryption")]
pub mod encryption;

pub mod errors;
pub use errors::*;

#[cfg(feature = "fft")]
pub mod fft;

#[cfg(feature = "hash_to_curve")]
pub mod hash_to_curve;

#[cfg(feature = "merkle_tree")]
pub mod merkle_tree;

#[cfg(feature = "msm")]
pub mod msm;

#[cfg(feature = "prf")]
pub mod prf;

#[cfg(feature = "signature")]
pub mod signature;

#[cfg(feature = "snark")]
pub mod snark;

pub mod traits;
pub use traits::*;

pub mod prelude {
    pub use crate::{errors::*, traits::*};
}
