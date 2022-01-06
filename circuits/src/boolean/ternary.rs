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

impl<E: Environment> Ternary for Boolean<E> {
    type Boolean = Boolean<E>;
    type Output = Self;

    /// Returns `first` if `condition` is `true`, otherwise returns `second`.
    fn ternary(condition: &Self::Boolean, first: &Self, second: &Self) -> Self::Output {
        // Constant `condition`
        if condition.is_constant() {
            match condition.eject_value() {
                true => first.clone(),
                false => second.clone(),
            }
        }
        // Constant `first`
        else if first.is_constant() {
            match first.eject_value() {
                true => condition.or(second),
                false => (!condition).and(second),
            }
        }
        // Constant `second`
        else if second.is_constant() {
            match second.eject_value() {
                true => (!condition).or(first),
                false => condition.and(first),
            }
        }
        // Variables
        else {
            let witness = Boolean::new(Mode::Private, match condition.eject_value() {
                true => first.eject_value(),
                false => second.eject_value(),
            });

            //
            // Ternary Enforcement
            // -------------------------------------------------------
            //    witness = condition * a + (1 - condition) * b
            // => witness = b + condition * (a - b)
            // => condition * (a - b) = witness - b
            //
            // See `Field::ternary()` for the proof of correctness.
            //
            E::enforce(|| (condition, (&first.0 - &second.0), (witness.0.clone() - &second.0)));

            witness
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Circuit;

    fn check_ternary(
        name: &str,
        expected: bool,
        condition: Boolean<Circuit>,
        a: Boolean<Circuit>,
        b: Boolean<Circuit>,
        num_constants: usize,
        num_public: usize,
        num_private: usize,
        num_constraints: usize,
    ) {
        Circuit::scoped(name, |scope| {
            let candidate = Boolean::ternary(&condition, &a, &b);
            assert_eq!(
                expected,
                candidate.eject_value(),
                "{} != {} := ({} ? {} : {})",
                expected,
                candidate.eject_value(),
                condition.eject_value(),
                a.eject_value(),
                b.eject_value()
            );

            assert_eq!(num_constants, scope.num_constants_in_scope());
            assert_eq!(num_public, scope.num_public_in_scope());
            assert_eq!(num_private, scope.num_private_in_scope());
            assert_eq!(num_constraints, scope.num_constraints_in_scope());
            assert!(Circuit::is_satisfied());
        });
    }

    #[test]
    fn test_constant_condition() {
        // false ? Constant : Constant
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Constant, false);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("false ? Constant : Constant", expected, condition, a, b, 0, 0, 0, 0);

        // false ? Constant : Public
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Constant, false);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("false ? Constant : Public", expected, condition, a, b, 0, 0, 0, 0);

        // false ? Public : Constant
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Constant, false);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("false ? Public : Constant", expected, condition, a, b, 0, 0, 0, 0);

        // false ? Public : Public
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Constant, false);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("false ? Public : Public", expected, condition, a, b, 0, 0, 0, 0);

        // false ? Public : Private
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Constant, false);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("false ? Public : Private", expected, condition, a, b, 0, 0, 0, 0);

        // false ? Private : Private
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Constant, false);
        let a = Boolean::<Circuit>::new(Mode::Private, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("false ? Private : Private", expected, condition, a, b, 0, 0, 0, 0);

        // true ? Constant : Constant
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Constant, true);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("true ? Constant : Constant", expected, condition, a, b, 0, 0, 0, 0);

        // true ? Constant : Public
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Constant, true);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("true ? Constant : Public", expected, condition, a, b, 0, 0, 0, 0);

        // true ? Public : Constant
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Constant, true);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("true ? Public : Constant", expected, condition, a, b, 0, 0, 0, 0);

        // true ? Public : Public
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Constant, true);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("true ? Public : Public", expected, condition, a, b, 0, 0, 0, 0);

        // true ? Public : Private
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Constant, true);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("true ? Public : Private", expected, condition, a, b, 0, 0, 0, 0);

        // true ? Private : Private
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Constant, true);
        let a = Boolean::<Circuit>::new(Mode::Private, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("true ? Private : Private", expected, condition, a, b, 0, 0, 0, 0);
    }

    #[test]
    fn test_public_condition_and_constant_input() {
        // false ? Constant : Constant
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Public, false);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("false ? Constant : Constant", expected, condition, a, b, 0, 0, 0, 0);

        // false ? Constant : Public
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Public, false);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("false ? Constant : Public", expected, condition, a, b, 0, 0, 1, 2);

        // false ? Public : Constant
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Public, false);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("false ? Public : Constant", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Constant : Constant
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Public, true);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("true ? Constant : Constant", expected, condition, a, b, 0, 0, 0, 0);

        // true ? Constant : Public
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Public, true);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("true ? Constant : Public", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Public : Constant
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Public, true);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("true ? Public : Constant", expected, condition, a, b, 0, 0, 1, 2);
    }

    #[test]
    fn test_private_condition_and_constant_input() {
        // false ? Constant : Constant
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Private, false);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("false ? Constant : Constant", expected, condition, a, b, 0, 0, 0, 0);

        // false ? Constant : Public
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Private, false);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("false ? Constant : Public", expected, condition, a, b, 0, 0, 1, 2);

        // false ? Public : Constant
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Private, false);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("false ? Public : Constant", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Constant : Constant
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Private, true);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("true ? Constant : Constant", expected, condition, a, b, 0, 0, 0, 0);

        // true ? Constant : Public
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Private, true);
        let a = Boolean::<Circuit>::new(Mode::Constant, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("true ? Constant : Public", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Public : Constant
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Private, true);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Constant, false);
        check_ternary("true ? Public : Constant", expected, condition, a, b, 0, 0, 1, 2);
    }

    #[test]
    fn test_public_condition_and_variable_inputs() {
        // false ? Public : Public
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Public, false);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("false ? Public : Public", expected, condition, a, b, 0, 0, 1, 2);

        // false ? Public : Private
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Public, false);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("false ? Public : Private", expected, condition, a, b, 0, 0, 1, 2);

        // false ? Private : Public
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Public, false);
        let a = Boolean::<Circuit>::new(Mode::Private, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("false ? Private : Public", expected, condition, a, b, 0, 0, 1, 2);

        // false ? Private : Private
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Public, false);
        let a = Boolean::<Circuit>::new(Mode::Private, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("false ? Private : Private", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Public : Public
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Public, true);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("true ? Public : Public", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Public : Private
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Public, true);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("true ? Public : Private", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Private : Public
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Public, true);
        let a = Boolean::<Circuit>::new(Mode::Private, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("true ? Private : Public", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Private : Private
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Public, true);
        let a = Boolean::<Circuit>::new(Mode::Private, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("true ? Private : Private", expected, condition, a, b, 0, 0, 1, 2);
    }

    #[test]
    fn test_private_condition_and_variable_inputs() {
        // false ? Public : Public
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Private, false);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("false ? Public : Public", expected, condition, a, b, 0, 0, 1, 2);

        // false ? Public : Private
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Private, false);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("false ? Public : Private", expected, condition, a, b, 0, 0, 1, 2);

        // false ? Private : Public
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Private, false);
        let a = Boolean::<Circuit>::new(Mode::Private, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("false ? Private : Public", expected, condition, a, b, 0, 0, 1, 2);

        // false ? Private : Private
        let expected = false;
        let condition = Boolean::<Circuit>::new(Mode::Private, false);
        let a = Boolean::<Circuit>::new(Mode::Private, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("false ? Private : Private", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Public : Public
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Private, true);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("true ? Public : Public", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Public : Private
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Private, true);
        let a = Boolean::<Circuit>::new(Mode::Public, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("true ? Public : Private", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Private : Public
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Private, true);
        let a = Boolean::<Circuit>::new(Mode::Private, true);
        let b = Boolean::<Circuit>::new(Mode::Public, false);
        check_ternary("true ? Private : Public", expected, condition, a, b, 0, 0, 1, 2);

        // true ? Private : Private
        let expected = true;
        let condition = Boolean::<Circuit>::new(Mode::Private, true);
        let a = Boolean::<Circuit>::new(Mode::Private, true);
        let b = Boolean::<Circuit>::new(Mode::Private, false);
        check_ternary("true ? Private : Private", expected, condition, a, b, 0, 0, 1, 2);
    }
}
