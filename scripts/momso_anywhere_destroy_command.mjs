import { GoogleBrowserProvider } from './momso_anywhere_google_browser.mjs';

export async function runDestroyCommand({ projectId, installationId, token, onProgress = () => {}, saveState, providerFactory = (value, options) => new GoogleBrowserProvider(value, options) }) {
  if (!token || /\s/.test(token) || typeof saveState !== 'function') throw new Error('invalid_authorization');
  if (!/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectId ?? '') || !/^[a-f0-9]{24}$/.test(installationId ?? '')) throw new Error('invalid_arguments');
  const provider = providerFactory(token, { projectId });
  await saveState({ projectId, installationId, remaining: ['unknown'] });
  const result = await provider.destroy(installationId, onProgress);
  await saveState({ projectId, installationId, remaining: result.remaining });
  return result;
}
