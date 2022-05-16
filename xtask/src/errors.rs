use std::fmt::Display;

#[derive(Debug)]
pub(crate) enum XError {
    EnumParse {
        type_name: &'static str,
        value: String,
    },
}

impl Display for XError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XError::EnumParse { type_name, value } => {
                write!(f, "Parse {type_name} from {value} failed.")
            }
        }
    }
}

impl std::error::Error for XError {}
