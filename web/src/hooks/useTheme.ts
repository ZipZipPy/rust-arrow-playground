import { useEffect } from 'react';
import { STORAGE_KEYS } from '../lib/storage';
import { usePersistentState } from './usePersistentState';

export type ThemeChoice = 'system' | 'light' | 'dark';

const ORDER: ThemeChoice[] = ['system', 'light', 'dark'];
const parseTheme = (raw: unknown): ThemeChoice => (ORDER.includes(raw as ThemeChoice) ? (raw as ThemeChoice) : 'system');

/** "system" follows prefers-color-scheme; light/dark pin the theme via data-theme on <html>. */
export function useTheme(): { theme: ThemeChoice; cycleTheme: () => void } {
  const [theme, setTheme] = usePersistentState<ThemeChoice>(STORAGE_KEYS.theme, parseTheme, 'system');

  useEffect(() => {
    const root = document.documentElement;
    if (theme === 'system') delete root.dataset.theme;
    else root.dataset.theme = theme;
  }, [theme]);

  const cycleTheme = () => setTheme((t) => ORDER[(ORDER.indexOf(t) + 1) % ORDER.length]!);
  return { theme, cycleTheme };
}
