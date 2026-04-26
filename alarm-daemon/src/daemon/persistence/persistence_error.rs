/// Errors raised by the persistence layer.
///
/// The variants intentionally avoid the umbrella `redb::Error` type so callers
/// can match on the specific failure mode (open vs commit vs decode vs …).
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// I/O error around the database file (path creation, etc.).
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to open or create the redb file.
    #[error("database open/create error: {0}")]
    Database(#[from] redb::DatabaseError),
    /// Failed to begin a read or write transaction.
    #[error("database transaction error: {0}")]
    Txn(#[from] redb::TransactionError),
    /// Failed to open the state table inside a transaction.
    #[error("database table error: {0}")]
    Table(#[from] redb::TableError),
    /// Failed to commit a write transaction.
    #[error("database commit error: {0}")]
    Commit(#[from] redb::CommitError),
    /// Failed to read a value from the state table.
    #[error("database read error: {0}")]
    Read(#[from] redb::StorageError),
    /// Stored payload could not be decoded — magic byte missing, schema
    /// version unrecognized, or body shorter than the layout requires.
    #[error("persistence corrupted: {0}")]
    Corrupted(String),
}
