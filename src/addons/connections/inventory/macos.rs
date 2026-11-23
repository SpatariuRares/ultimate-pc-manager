//! macOS: the connection tables are not read yet.

use super::{assemble, Inventory, TableError};

/// Reads the four tables of this PC: none can be read yet.
pub fn read() -> Inventory {
    assemble([Err(TableError), Err(TableError), Err(TableError), Err(TableError)])
}
