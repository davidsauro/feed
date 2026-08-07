//! This server's identity, which has to be the same every time it starts.
//!
//! Clients are configured with an address that includes the server's public key,
//! and refuse to talk to anything that can't prove it holds the matching private
//! key. That is what stops a hijacked hostname impersonating a server — and it
//! also means a server which generates a fresh identity on every start is a
//! server nobody can find twice.

use std::fs;
use std::path::Path;

use libp2p::identity::Keypair;

/// Loads the identity from disk, creating one on first run.
pub fn load_or_create(path: &Path) -> Result<Keypair, String> {
    if let Some(directory) = path.parent() {
        if !directory.as_os_str().is_empty() {
            fs::create_dir_all(directory)
                .map_err(|error| format!("could not create {}: {}", directory.display(), error))?;
        }
    }

    if path.exists() {
        let bytes = fs::read(path)
            .map_err(|error| format!("could not read {}: {}", path.display(), error))?;

        let keypair = Keypair::from_protobuf_encoding(&bytes)
            .map_err(|error| format!("{} is not a usable identity: {}", path.display(), error))?;

        restrict_to_owner(path);

        return Ok(keypair);
    }

    let keypair = Keypair::generate_ed25519();
    let bytes = keypair
        .to_protobuf_encoding()
        .map_err(|error| format!("could not encode a new identity: {}", error))?;

    fs::write(path, bytes)
        .map_err(|error| format!("could not write {}: {}", path.display(), error))?;
    restrict_to_owner(path);

    println!("created a new identity at {}", path.display());

    Ok(keypair)
}

/// Keeps the private key to the account running the server.
///
/// Anyone who copies this file can be this server: they can accept its clients'
/// connections and see everything it carries. Reported rather than fatal, since
/// a server that refuses to start over file permissions is worse than one that
/// complains.
fn restrict_to_owner(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            eprintln!(
                "could not restrict permissions on {}: {}",
                path.display(),
                error
            );
        }
    }

    #[cfg(not(unix))]
    let _ = path;
}
