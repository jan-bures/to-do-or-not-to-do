//! Importer skeleton: will read a Microsoft Graph export plus a copy of the local To Do cache and
//! produce an import bundle for the server.

fn main() -> anyhow::Result<()> {
    println!("todo-import (protocol {})", todo_core::PROTOCOL_VERSION);
    Ok(())
}
