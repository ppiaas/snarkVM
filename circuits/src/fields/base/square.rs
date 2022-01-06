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

use super::*;

impl<E: Environment> Square for BaseField<E> {
    type Output = BaseField<E>;

    fn square(&self) -> Self::Output {
        (&self).square()
    }
}

impl<E: Environment> Square for &BaseField<E> {
    type Output = BaseField<E>;

    fn square(&self) -> Self::Output {
        *self * *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Circuit;

    const ITERATIONS: usize = 500;

    #[test]
    fn test_square() {
        let one = <Circuit as Environment>::BaseField::one();

        // Constant variables
        Circuit::scoped("Constant", |scope| {
            let mut expected = one;
            let mut candidate = BaseField::<Circuit>::new(Mode::Constant, one);

            for _ in 0..ITERATIONS {
                expected = expected.square();
                candidate = candidate.square();
                assert_eq!(expected, candidate.eject_value());

                assert_eq!(1, scope.num_constants_in_scope());
                assert_eq!(0, scope.num_public_in_scope());
                assert_eq!(0, scope.num_private_in_scope());
                assert_eq!(0, scope.num_constraints_in_scope());
            }
        });

        // Public variables
        Circuit::scoped("Public", |scope| {
            let mut expected = one;
            let mut candidate = BaseField::<Circuit>::new(Mode::Public, one);

            for i in 0..ITERATIONS {
                expected = expected.square();
                candidate = candidate.square();
                assert_eq!(expected, candidate.eject_value());

                assert_eq!(0, scope.num_constants_in_scope());
                assert_eq!(1, scope.num_public_in_scope());
                assert_eq!(i + 1, scope.num_private_in_scope());
                assert_eq!(i + 1, scope.num_constraints_in_scope());
                assert!(scope.is_satisfied());
            }
        });

        // Private variables
        Circuit::scoped("Private", |scope| {
            let mut expected = one;
            let mut candidate = BaseField::<Circuit>::new(Mode::Private, one);

            for i in 0..ITERATIONS {
                expected = expected.square();
                candidate = candidate.square();
                assert_eq!(expected, candidate.eject_value());

                assert_eq!(0, scope.num_constants_in_scope());
                assert_eq!(0, scope.num_public_in_scope());
                assert_eq!(i + 2, scope.num_private_in_scope());
                assert_eq!(i + 1, scope.num_constraints_in_scope());
                assert!(scope.is_satisfied());
            }
        });
    }

    #[test]
    fn test_0_square() {
        let zero = <Circuit as Environment>::BaseField::zero();

        let candidate = BaseField::<Circuit>::new(Mode::Public, zero).square();
        assert_eq!(zero, candidate.eject_value());
    }

    #[test]
    fn test_1_double() {
        let one = <Circuit as Environment>::BaseField::one();

        let candidate = BaseField::<Circuit>::new(Mode::Public, one).square();
        assert_eq!(one, candidate.eject_value());
    }

    #[test]
    fn test_2_double() {
        let one = <Circuit as Environment>::BaseField::one();
        let two = one + one;
        let four = two.square();

        let candidate = (BaseField::<Circuit>::new(Mode::Public, one) + BaseField::new(Mode::Public, one)).square();
        assert_eq!(four, candidate.eject_value());
    }
}
