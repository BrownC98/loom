use super::errors::Refusal;

impl From<rusqlite::Error> for Refusal {
    fn from(e: rusqlite::Error) -> Self {
        Refusal::Db(e.to_string())
    }
}
