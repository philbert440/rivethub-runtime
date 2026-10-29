use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaVersion {
    V1 = 1,
    V2 = 2,
    V3 = 3,
    V4 = 4,
    V5 = 5,
    V6 = 6,
    V7 = 7,
    V8 = 8,
    V9 = 9,
    V10 = 10,
    V11 = 11,
    V12 = 12,
    V13 = 13,
    V14 = 14,
    V15 = 15,
    V16 = 16,
    V17 = 17,
    V18 = 18,
    V19 = 19,
}

impl SchemaVersion {
    #[must_use]
    pub fn number(self) -> i64 {
        self as i64
    }

    #[must_use]
    pub fn next(self) -> Option<Self> {
        match self {
            Self::V1 => Some(Self::V2),
            Self::V2 => Some(Self::V3),
            Self::V3 => Some(Self::V4),
            Self::V4 => Some(Self::V5),
            Self::V5 => Some(Self::V6),
            Self::V6 => Some(Self::V7),
            Self::V7 => Some(Self::V8),
            Self::V8 => Some(Self::V9),
            Self::V9 => Some(Self::V10),
            Self::V10 => Some(Self::V11),
            Self::V11 => Some(Self::V12),
            Self::V12 => Some(Self::V13),
            Self::V13 => Some(Self::V14),
            Self::V14 => Some(Self::V15),
            Self::V15 => Some(Self::V16),
            Self::V16 => Some(Self::V17),
            Self::V17 => Some(Self::V18),
            Self::V18 => Some(Self::V19),
            Self::V19 => None,
        }
    }
}

impl TryFrom<i64> for SchemaVersion {
    type Error = Error;

    fn try_from(found: i64) -> Result<Self, Self::Error> {
        match found {
            1 => Ok(Self::V1),
            2 => Ok(Self::V2),
            3 => Ok(Self::V3),
            4 => Ok(Self::V4),
            5 => Ok(Self::V5),
            6 => Ok(Self::V6),
            7 => Ok(Self::V7),
            8 => Ok(Self::V8),
            9 => Ok(Self::V9),
            10 => Ok(Self::V10),
            11 => Ok(Self::V11),
            12 => Ok(Self::V12),
            13 => Ok(Self::V13),
            14 => Ok(Self::V14),
            15 => Ok(Self::V15),
            16 => Ok(Self::V16),
            17 => Ok(Self::V17),
            18 => Ok(Self::V18),
            19 => Ok(Self::V19),
            other => Err(Error::Unsupported { found: other }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SchemaVersion;
    use crate::error::Error;

    #[test]
    fn every_recorded_user_version_maps_to_a_schema_version() {
        for found in 1..=19 {
            let version = SchemaVersion::try_from(found).unwrap_or_else(|error| {
                panic!("user_version {found} must map to a schema version: {error}")
            });
            assert_eq!(version.number(), found);
        }
    }

    #[test]
    fn versions_outside_the_ladder_are_typed_errors() {
        for found in [-3, 0, 20, 99] {
            match SchemaVersion::try_from(found) {
                Err(Error::Unsupported { found: reported }) => {
                    assert_eq!(reported, found);
                }
                other => panic!("user_version {found} must be unsupported, got {other:?}"),
            }
        }
    }

    #[test]
    fn the_ladder_runs_from_any_version_to_the_nineteenth() {
        for from in 1..=19 {
            let mut current = SchemaVersion::try_from(from).unwrap();
            let mut visited = Vec::new();
            while let Some(next) = current.next() {
                visited.push(next.number());
                current = next;
            }
            let expected: Vec<i64> = (from + 1..=19).collect();
            assert_eq!(visited, expected, "ladder from user_version {from}");
        }
    }
}
