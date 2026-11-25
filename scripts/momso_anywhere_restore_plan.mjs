// Only verified observations may distinguish a fresh install from a preserved backup.
import { SetupError } from './momso_anywhere_setup.mjs';

export function restorePlan({ vault, bucket, key, slots }) {
  const resources = [vault, bucket, key];
  if (resources.some(value => !['absent', 'ready', 'unverified'].includes(value))) throw new SetupError('restore_assets_unverified');
  if (resources.includes('unverified')) throw new SetupError('restore_assets_unverified');
  if (resources.every(value => value === 'absent')) return { restoreAvailable: false, lastBackupAt: null, requiresRestore: false };
  if (resources.some(value => value !== 'ready')) throw new SetupError('restore_assets_incomplete');
  if (!Array.isArray(slots)) throw new SetupError('restore_assets_unverified');
  const found = slots.filter(item => /^momso-backup\/slot-[0-6]\.enc$/.test(item?.name ?? ''));
  if (!found.length) return { restoreAvailable: false, lastBackupAt: null, requiresRestore: true };
  const dates = found.map(item => {
    if (typeof item.updated !== 'string' || !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)$/.test(item.updated) || !Number.isFinite(Date.parse(item.updated))) throw new SetupError('restore_assets_unverified');
    const [year, month, day] = item.updated.slice(0, 10).split('-').map(Number);
    const calendar = new Date(0); calendar.setUTCFullYear(year, month - 1, day);
    if (year < 1 || calendar.getUTCFullYear() !== year || calendar.getUTCMonth() !== month - 1 || calendar.getUTCDate() !== day) throw new SetupError('restore_assets_unverified');
    return { value: item.updated, time: Date.parse(item.updated) };
  });
  dates.sort((a, b) => b.time - a.time);
  return { restoreAvailable: true, lastBackupAt: dates[0].value, requiresRestore: true };
}
