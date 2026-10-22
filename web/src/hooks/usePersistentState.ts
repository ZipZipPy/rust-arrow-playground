import { type Dispatch, type SetStateAction, useEffect, useState } from 'react';
import { readStored, writeStored } from '../lib/storage';

/** useState mirrored to localStorage; falls back to `fallback` when storage is unavailable. */
export function usePersistentState<T>(
  key: string,
  parse: (raw: unknown) => T,
  fallback: T,
): [T, Dispatch<SetStateAction<T>>] {
  const [value, setValue] = useState<T>(() => readStored(key, parse, fallback));
  useEffect(() => writeStored(key, value), [key, value]);
  return [value, setValue];
}
