//! Port of libs/librepcb/core/sqlitedatabase.{h,cpp}: a thin wrapper around
//! a [`rusqlite::Connection`] (with bundled SQLite) as used by the
//! workspace library index.
//!
//! Differences to upstream:
//! - Queries are plain [`rusqlite`] statements with named parameters
//!   (`:name`), prepared by [`SqliteDatabase::prepare()`] and executed with
//!   rusqlite's API or the [`count()`](SqliteDatabase::count) and
//!   [`insert()`](SqliteDatabase::insert) helpers.
//! - `TransactionScopeGuard` is [`rusqlite::Transaction`] (see
//!   [`SqliteDatabase::transaction()`]), which rolls back on drop unless
//!   committed.

use std::path::{Path, PathBuf};

use foilfern_i18n::tr;
use rusqlite::{CachedStatement, Connection, Params, Statement, Transaction};

/// Result type of the SQLite database wrapper.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Error of [`SqliteDatabase`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The database could not be opened (or created).
    #[error(
        "{}",
        tr!("librepcb::SQLiteDatabase", "Could not open database: \"{0}\"", .path.display())
    )]
    Open {
        /// Path of the database file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: rusqlite::Error,
    },
    /// Write-Ahead Logging could not be enabled; contains the journal mode
    /// reported by SQLite.
    #[error("Could not enable SQLite Write-Ahead Logging: {0}")]
    WalNotEnabled(String),
    /// A query could not be prepared or executed.
    #[error("Error while executing SQL query: {query}\n{source}")]
    Query {
        /// The SQL query.
        query: String,
        /// The underlying error.
        #[source]
        source: rusqlite::Error,
    },
    /// Any other SQLite error.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// A connection to an SQLite database file.
///
/// On opening, foreign keys are enabled and the journal mode is set to WAL
/// ("Write-Ahead Logging"), which LibrePCB requires to avoid blocking
/// readers by writers (e.g. the library scanner would otherwise block all
/// read accesses to the library index).
#[derive(Debug)]
pub struct SqliteDatabase {
    connection: Connection,
}

// Deliberately `Send` but not `Sync` (like `rusqlite::Connection`): a
// connection is moved to the thread using it (e.g. the library scanner);
// wrap it in a `Mutex` to share one, or open one connection per thread
// (WAL mode allows concurrent readers).
static_assertions::assert_impl_all!(SqliteDatabase: Send);
static_assertions::assert_not_impl_any!(SqliteDatabase: Sync);

impl SqliteDatabase {
    /// Opens the database, creating the file if it doesn't exist.
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path).map_err(|source| Error::Open {
            path: path.to_owned(),
            source,
        })?;
        let db = Self { connection };
        db.exec("PRAGMA foreign_keys = ON")?;
        db.enable_write_ahead_logging()?;
        Ok(db)
    }

    /// Returns the underlying connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Starts a transaction, which is rolled back when dropped unless
    /// [`Transaction::commit()`] is called.
    ///
    /// Queries within the transaction can be executed through `self` or the
    /// returned transaction (which dereferences to [`Connection`]).
    pub fn transaction(&self) -> Result<Transaction<'_>> {
        Ok(self.connection.unchecked_transaction()?)
    }

    /// Removes all rows from the given table.
    pub fn clear_table(&self, table: &str) -> Result<()> {
        self.exec(&format!("DELETE FROM {table}"))
    }

    /// Prepares (and caches) a query after applying textual `replacements`
    /// (in the given order), e.g. `("%elements", "devices")` to reuse the
    /// same query for several tables.
    pub fn prepare(
        &self,
        query: &str,
        replacements: &[(&str, &str)],
    ) -> Result<CachedStatement<'_>> {
        let query = replacements
            .iter()
            .fold(query.to_owned(), |q, (from, to)| q.replace(from, to));
        self.connection
            .prepare_cached(&query)
            .map_err(|source| Error::Query { query, source })
    }

    /// Executes a query which returns a single integer (e.g. `COUNT(*)`) and
    /// returns that value.
    pub fn count(&self, statement: &mut Statement<'_>, params: impl Params) -> Result<i64> {
        Ok(statement.query_row(params, |row| row.get(0))?)
    }

    /// Executes an `INSERT` query and returns the row ID of the inserted
    /// row.
    pub fn insert(&self, statement: &mut Statement<'_>, params: impl Params) -> Result<i64> {
        statement.execute(params)?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Executes an SQL query without parameters (results are discarded).
    pub fn exec(&self, query: &str) -> Result<()> {
        self.connection
            .execute_batch(query)
            .map_err(|source| Error::Query {
                query: query.to_owned(),
                source,
            })
    }

    fn enable_write_ahead_logging(&self) -> Result<()> {
        let mode: String = self
            .connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        if mode == "wal" {
            Ok(())
        } else {
            Err(Error::WalNotEnabled(mode))
        }
    }
}
