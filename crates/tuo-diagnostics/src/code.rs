//! Stable, namespaced diagnostic codes.

use std::error::Error;
use std::fmt;
use std::str::FromStr;

/// The pipeline stage a diagnostic code belongs to.
///
/// Each namespace owns the four-digit code space after its letter (`L0000`
/// through `L9999`, and so on). The eight namespaces below are **reserved**;
/// adding a stage means adding a variant here, never overloading a letter.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Namespace {
    /// `Lxxxx` — lexical analysis.
    Lexical,
    /// `Pxxxx` — parsing.
    Parser,
    /// `Rxxxx` — name/path resolution.
    Resolution,
    /// `Txxxx` — type checking and inference.
    Type,
    /// `Oxxxx` — ownership and memory-safety analysis.
    Ownership,
    /// `Mxxxx` — MIR construction and checks.
    Mir,
    /// `Sxxxx` — executable specifications.
    Specification,
    /// `Cxxxx` — code generation.
    Codegen,
}

impl Namespace {
    /// All reserved namespaces, in pipeline order.
    pub const ALL: [Self; 8] = [
        Self::Lexical,
        Self::Parser,
        Self::Resolution,
        Self::Type,
        Self::Ownership,
        Self::Mir,
        Self::Specification,
        Self::Codegen,
    ];

    /// The namespace's code-prefix letter.
    #[must_use]
    pub const fn letter(self) -> char {
        match self {
            Self::Lexical => 'L',
            Self::Parser => 'P',
            Self::Resolution => 'R',
            Self::Type => 'T',
            Self::Ownership => 'O',
            Self::Mir => 'M',
            Self::Specification => 'S',
            Self::Codegen => 'C',
        }
    }

    /// The namespace for a prefix letter, if it is reserved.
    #[must_use]
    pub const fn from_letter(letter: char) -> Option<Self> {
        Some(match letter {
            'L' => Self::Lexical,
            'P' => Self::Parser,
            'R' => Self::Resolution,
            'T' => Self::Type,
            'O' => Self::Ownership,
            'M' => Self::Mir,
            'S' => Self::Specification,
            'C' => Self::Codegen,
            _ => return None,
        })
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.letter())
    }
}

/// A stable diagnostic code: a [`Namespace`] letter plus a four-digit number,
/// e.g. `T0301`.
///
/// Codes are the machine-facing identity of an error class. They are
/// **stable**: once a code has shipped with a meaning, that number is never
/// reused for a different error (retired codes stay retired).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DiagnosticCode {
    namespace: Namespace,
    number: u16,
}

/// Error: a string was not a valid diagnostic code.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InvalidCode {
    /// The rejected input.
    pub input: String,
}

impl fmt::Display for InvalidCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid diagnostic code `{}`: expected a reserved namespace letter \
             (L, P, R, T, O, M, S, C) followed by four digits",
            self.input
        )
    }
}

impl Error for InvalidCode {}

impl DiagnosticCode {
    /// Construct a code from its namespace and number.
    ///
    /// # Panics
    ///
    /// Panics if `number` exceeds the four-digit space (9999); code numbers
    /// are assigned by hand and an overflow is a programming error.
    #[must_use]
    pub fn new(namespace: Namespace, number: u16) -> Self {
        assert!(
            number <= 9999,
            "diagnostic code number {number} exceeds the four-digit space"
        );
        Self { namespace, number }
    }

    /// The code's namespace.
    #[must_use]
    pub const fn namespace(self) -> Namespace {
        self.namespace
    }

    /// The code's number within its namespace.
    #[must_use]
    pub const fn number(self) -> u16 {
        self.number
    }
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{:04}", self.namespace.letter(), self.number)
    }
}

impl FromStr for DiagnosticCode {
    type Err = InvalidCode;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || InvalidCode {
            input: s.to_owned(),
        };
        let mut chars = s.chars();
        let namespace = chars
            .next()
            .and_then(Namespace::from_letter)
            .ok_or_else(invalid)?;
        let digits = chars.as_str();
        if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid());
        }
        let number = digits.parse::<u16>().map_err(|_| invalid())?;
        Ok(Self { namespace, number })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_render_with_zero_padding() {
        assert_eq!(DiagnosticCode::new(Namespace::Type, 1).to_string(), "T0001");
        assert_eq!(
            DiagnosticCode::new(Namespace::Codegen, 9999).to_string(),
            "C9999"
        );
    }

    #[test]
    fn all_reserved_namespaces_round_trip_their_letter() {
        for ns in Namespace::ALL {
            assert_eq!(Namespace::from_letter(ns.letter()), Some(ns));
            let code = DiagnosticCode::new(ns, 42);
            assert_eq!(code.to_string().parse::<DiagnosticCode>(), Ok(code));
        }
    }

    #[test]
    fn unreserved_letters_and_malformed_codes_are_rejected() {
        for bad in ["X0001", "T001", "T00001", "T00a1", "", "0001", "t0001"] {
            assert!(bad.parse::<DiagnosticCode>().is_err(), "{bad} must fail");
        }
    }

    #[test]
    #[should_panic(expected = "exceeds the four-digit space")]
    fn five_digit_numbers_panic() {
        let _ = DiagnosticCode::new(Namespace::Lexical, 10_000);
    }
}
