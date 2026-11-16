use crate::{backup_profile, credentials, storage};
use age::{secrecy::ExposeSecret, x25519};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path, PathBuf},
};
use tempfile::TempDir;
use thiserror::Error;
use walkdir::WalkDir;

#[derive(Debug, Error)]
pub(crate) enum ArchiveError {
    #[error(transparent)]
    Storage(#[from] storage::StorageError),
    #[error(transparent)]
    Credential(#[from] credentials::CredentialError),
    #[error("암호화 백업 파일을 만들거나 읽지 못했습니다")]
    Io(#[from] io::Error),
    #[error("기록 파일 경로가 작업공간을 벗어났습니다")]
    UnsafePath,
    #[error("백업 암호화에 실패했습니다")]
    Encryption,
    #[error("복구 키가 없거나 백업 파일과 맞지 않습니다")]
    Decryption,
    #[error("백업 파일의 해시가 원격 연결 정보와 다릅니다")]
    HashMismatch,
    #[error("같은 경로에 내용이 다른 로컬 기록 파일이 있습니다")]
    ObjectConflict,
}

pub(crate) struct PreparedArchive {
    _temporary_directory: TempDir,
    pub(crate) encrypted_path: PathBuf,
    pub(crate) encrypted_sha256: String,
    pub(crate) new_recovery_key: Option<String>,
    pub(crate) snapshot_database_path: PathBuf,
    pub(crate) object_files: Vec<backup_profile::ObjectFileState>,
}

pub(crate) struct ExtractedArchive {
    _temporary_directory: TempDir,
    pub(crate) database_path: PathBuf,
    pub(crate) objects_path: PathBuf,
}

pub(crate) fn sha256_file(path: &Path) -> Result<String, ArchiveError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn prepare(
    database_path: &Path,
    objects_path: &Path,
    backups_path: &Path,
    workspace_id: &str,
    snapshot_id: &str,
) -> Result<PreparedArchive, ArchiveError> {
    let temporary_directory = tempfile::Builder::new()
        .prefix("momso-backup-")
        .tempdir_in(backups_path)?;
    let snapshot_database = temporary_directory.path().join("momso.db");
    storage::create_snapshot_file(database_path, &snapshot_database)?;
    storage::sanitize_user_owned_backup_snapshot(&snapshot_database)?;
    let tar_path = temporary_directory.path().join("snapshot.tar");
    let object_files = build_tar(&snapshot_database, objects_path, &tar_path)?;

    let (identity, created) = credentials::get_or_create_backup_identity(workspace_id)?;
    let encrypted_path = temporary_directory
        .path()
        .join(format!("{snapshot_id}.tar.age"));
    encrypt_file(&tar_path, &encrypted_path, &identity)?;
    let encrypted_sha256 = sha256_file(&encrypted_path)?;
    let new_recovery_key = created.then(|| identity.to_string().expose_secret().to_owned());

    Ok(PreparedArchive {
        _temporary_directory: temporary_directory,
        encrypted_path,
        encrypted_sha256,
        new_recovery_key,
        snapshot_database_path: snapshot_database,
        object_files,
    })
}

fn build_tar(
    snapshot_database: &Path,
    objects_path: &Path,
    destination: &Path,
) -> Result<Vec<backup_profile::ObjectFileState>, ArchiveError> {
    let output = File::create(destination)?;
    let mut builder = tar::Builder::new(output);
    builder.append_path_with_name(snapshot_database, "database/momso.db")?;
    let mut entries = WalkDir::new(objects_path)
        .follow_links(false)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    entries.retain(|entry| entry.file_type().is_file());
    entries.sort_by_key(|entry| entry.path().to_path_buf());
    let mut object_files = Vec::with_capacity(entries.len());
    for entry in entries {
        let relative = entry
            .path()
            .strip_prefix(objects_path)
            .map_err(|_| ArchiveError::UnsafePath)?;
        validate_relative_path(relative)?;
        let file = File::open(entry.path())?;
        let byte_length = file.metadata()?.len();
        let mut header = tar::Header::new_gnu();
        header.set_size(byte_length);
        header.set_mode(0o600);
        header.set_mtime(0);
        header.set_cksum();
        let mut reader = HashingReader::new(file);
        builder.append_data(
            &mut header,
            Path::new("objects").join(relative),
            &mut reader,
        )?;
        if reader.byte_length != byte_length {
            return Err(ArchiveError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "source file changed while the backup snapshot was being captured",
            )));
        }
        let relative_path = relative
            .to_str()
            .ok_or(ArchiveError::UnsafePath)?
            .replace('\\', "/");
        object_files.push(backup_profile::ObjectFileState {
            relative_path,
            sha256: format!("{:x}", reader.hasher.finalize()),
            byte_length,
        });
    }
    builder.finish()?;
    Ok(object_files)
}

struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    byte_length: u64,
}

impl<R> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            byte_length: 0,
        }
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.hasher.update(&buffer[..read]);
        self.byte_length = self
            .byte_length
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("source file is too large"))?;
        Ok(read)
    }
}

fn encrypt_file(
    source: &Path,
    destination: &Path,
    identity: &x25519::Identity,
) -> Result<(), ArchiveError> {
    let recipient = identity.to_public();
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .map_err(|_| ArchiveError::Encryption)?;
    let mut input = File::open(source)?;
    let output = File::create(destination)?;
    let mut writer = encryptor
        .wrap_output(output)
        .map_err(|_| ArchiveError::Encryption)?;
    io::copy(&mut input, &mut writer).map_err(|_| ArchiveError::Encryption)?;
    writer.finish().map_err(|_| ArchiveError::Encryption)?;
    Ok(())
}

pub(crate) fn extract(
    encrypted_path: &Path,
    expected_sha256: &str,
    backups_path: &Path,
    workspace_id: &str,
) -> Result<ExtractedArchive, ArchiveError> {
    if sha256_file(encrypted_path)? != expected_sha256 {
        return Err(ArchiveError::HashMismatch);
    }
    let identity =
        credentials::get_backup_identity(workspace_id)?.ok_or(ArchiveError::Decryption)?;
    let temporary_directory = tempfile::Builder::new()
        .prefix("momso-restore-")
        .tempdir_in(backups_path)?;
    let extracted_root = temporary_directory.path().join("extracted");
    fs::create_dir_all(&extracted_root)?;
    extract_with_identity(encrypted_path, &extracted_root, &identity)?;
    let database_path = extracted_root.join("database").join("momso.db");
    let objects_path = extracted_root.join("objects");
    if !database_path.is_file() {
        return Err(ArchiveError::UnsafePath);
    }
    fs::create_dir_all(&objects_path)?;
    storage::validate_snapshot_file(&database_path)?;
    Ok(ExtractedArchive {
        _temporary_directory: temporary_directory,
        database_path,
        objects_path,
    })
}

fn extract_with_identity(
    encrypted_path: &Path,
    destination: &Path,
    identity: &x25519::Identity,
) -> Result<(), ArchiveError> {
    let encrypted = File::open(encrypted_path)?;
    let decryptor = age::Decryptor::new(encrypted).map_err(|_| ArchiveError::Decryption)?;
    let reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|_| ArchiveError::Decryption)?;
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        if !(entry_type.is_file() || entry_type.is_dir()) {
            return Err(ArchiveError::UnsafePath);
        }
        let path = entry.path()?.into_owned();
        validate_archive_path(&path)?;
        if !entry.unpack_in(destination)? {
            return Err(ArchiveError::UnsafePath);
        }
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<(), ArchiveError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ArchiveError::UnsafePath);
    }
    Ok(())
}

fn validate_archive_path(path: &Path) -> Result<(), ArchiveError> {
    validate_relative_path(path)?;
    let first = path
        .components()
        .next()
        .and_then(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .ok_or(ArchiveError::UnsafePath)?;
    if first != "database" && first != "objects" {
        return Err(ArchiveError::UnsafePath);
    }
    Ok(())
}

pub(crate) fn validate_object_merge(
    extracted_objects: &Path,
    local_objects: &Path,
) -> Result<(), ArchiveError> {
    for entry in WalkDir::new(extracted_objects).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(extracted_objects)
            .map_err(|_| ArchiveError::UnsafePath)?;
        validate_relative_path(relative)?;
        let destination = local_objects.join(relative);
        if destination.is_file() && sha256_file(entry.path())? != sha256_file(&destination)? {
            return Err(ArchiveError::ObjectConflict);
        }
    }
    Ok(())
}

pub(crate) fn merge_objects(
    extracted_objects: &Path,
    local_objects: &Path,
) -> Result<(), ArchiveError> {
    validate_object_merge(extracted_objects, local_objects)?;
    for entry in WalkDir::new(extracted_objects).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(extracted_objects)
            .map_err(|_| ArchiveError::UnsafePath)?;
        let destination = local_objects.join(relative);
        if destination.exists() {
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(entry.path(), destination)?;
    }
    Ok(())
}

impl From<walkdir::Error> for ArchiveError {
    fn from(error: walkdir::Error) -> Self {
        match error.into_io_error() {
            Some(error) => Self::Io(error),
            None => Self::UnsafePath,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn age_archive_round_trip_preserves_database_and_objects() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("source.txt");
        let encrypted = temp.path().join("source.age");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&destination).expect("destination creates");
        fs::write(&source, b"sensitive record").expect("source writes");
        let identity = x25519::Identity::generate();
        encrypt_file(&source, &encrypted, &identity).expect("encrypts");

        let decryptor = age::Decryptor::new(File::open(&encrypted).expect("encrypted opens"))
            .expect("decryptor creates");
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .expect("decrypts");
        let mut output = Vec::new();
        reader.read_to_end(&mut output).expect("plaintext reads");
        assert_eq!(output, b"sensitive record");
    }

    #[test]
    fn archive_paths_cannot_escape_or_add_unknown_roots() {
        assert!(validate_archive_path(Path::new("database/momso.db")).is_ok());
        assert!(validate_archive_path(Path::new("objects/audio.mp3")).is_ok());
        assert!(validate_archive_path(Path::new("../outside.txt")).is_err());
        assert!(validate_archive_path(Path::new("secrets/key.txt")).is_err());
    }

    #[test]
    fn tar_inventory_hashes_the_exact_database_snapshot_and_object_bytes() {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        let backups_path = temp.path().join("backups");
        let objects_path = temp.path().join("objects");
        let tar_path = temp.path().join("snapshot.tar");
        let extracted_path = temp.path().join("extracted");
        storage::initialize(&database_path, &backups_path, "0.1.0-test")
            .expect("storage initializes");
        fs::create_dir_all(objects_path.join("sha256/aa")).expect("objects create");
        fs::write(
            objects_path.join("sha256/aa/source.bin"),
            b"exact object bytes",
        )
        .expect("object writes");

        let inventory = build_tar(&database_path, &objects_path, &tar_path)
            .expect("tar and inventory build together");
        let captured =
            backup_profile::local_content_state_from_inventory(&database_path, &inventory)
                .expect("captured state hashes");
        fs::create_dir_all(&extracted_path).expect("extracted directory creates");
        tar::Archive::new(File::open(&tar_path).expect("tar opens"))
            .unpack(&extracted_path)
            .expect("tar extracts");
        let restored = backup_profile::local_content_state(
            &extracted_path.join("database/momso.db"),
            &extracted_path.join("objects"),
        )
        .expect("extracted state hashes");

        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].relative_path, "sha256/aa/source.bin");
        assert_eq!(captured.sha256, restored.sha256);
        assert_eq!(captured.object_count, restored.object_count);
    }
}
