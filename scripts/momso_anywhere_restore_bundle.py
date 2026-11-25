"""Read restore workers only from the already verified signed VM bundle."""
import io
import tarfile

from momso_anywhere_restore_files import RESTORE_FILES


def restore_files(bundle, *, include_vault=False):
    wanted = set(RESTORE_FILES)
    if include_vault:
        wanted.add('vault-provision.py')
    result = {}
    with tarfile.open(fileobj=io.BytesIO(bundle), mode='r:gz') as archive:
        for member in archive:
            name = member.name
            if name not in wanted:
                continue
            if name in result or not member.isfile() or member.size > 8 * 1024 * 1024:
                raise ValueError('restore_bundle_invalid')
            stream = archive.extractfile(member)
            if stream is None:
                raise ValueError('restore_bundle_invalid')
            value = stream.read(member.size + 1)
            if len(value) != member.size:
                raise ValueError('restore_bundle_invalid')
            result[name] = value
    if set(result) != wanted:
        raise ValueError('restore_bundle_invalid')
    return result
