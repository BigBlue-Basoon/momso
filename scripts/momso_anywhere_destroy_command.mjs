import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';

export async function runDestroyCommand({ projectId, installationId, token, saveState, previous = null, discardData = false, onProgress = () => {}, providerFactory = (value, options) => new GoogleBrowserProvider(value, options) }) {
  if (!token || /\s/.test(token) || typeof saveState !== 'function') throw new Error('invalid_authorization');
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') || !/^[a-f0-9]{24}$/.test(installationId ?? '')) throw new Error('invalid_arguments');
  // No maintenance-and-backup proof can currently be supplied by this CLI.
  // Refuse before creating a provider or writing a misleading checkpoint.
  if (discardData !== true) throw Object.assign(new Error('backup_freshness_unverified'), { step: 'backup_freshness_unverified' });
  const provider = providerFactory(token, { projectId });
  await saveState({ projectId, installationId, remaining: ['unknown'], deletion: previous });
  const infrastructure = await provider.destroy(installationId, onProgress, { discardData: true });
  const deletion = await provider.discardBackup(installationId, previous);
  for (const name of ['vm', 'gateway', 'network', 'secrets', 'iam']) deletion.resources[name] ??= { status: 'unverified', reason: 'not_inventoried' };
  const done = ['bucket', 'kms', 'secret_vault'].every(name => deletion.resources?.[name]?.status === 'deleted');
  await saveState({ projectId, installationId, remaining: infrastructure.remaining, deletion });
  return { ...deletion, remaining: infrastructure.remaining, status: done && !infrastructure.remaining.length ? 'complete' : 'pending' };
}
