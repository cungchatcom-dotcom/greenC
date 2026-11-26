// ui/src/lib/cache.ts: show the last result at once, refresh it in the background.
//
// Scans take seconds to minutes. Each screen keeps its latest result (in memory and in the
// WebView's localStorage) and shows it immediately while a new scan runs. A shown result is
// only "fresh" when it comes from a scan made now; screens disable actions on files until then.

declare const __APP_VERSION__: string;
// A new app version may change the shape of results, so each version has its own entries.
const VERSION = typeof __APP_VERSION__ === 'string' ? __APP_VERSION__ : 'dev';
const FAMILY = 'greenc.cache.';
const PREFIX = `${FAMILY}${VERSION}.`;

export interface Cached<T> {
  data: T;
  savedAt: number;
}

export interface RefreshOptions<T> {
  /** Start a new scan even if one is running (use after changing files). */
  force?: boolean;
  /** Shrinks a result before it is written to storage. Memory keeps the full one. */
  compact?: (data: T) => T;
}

export interface LoadOptions<T> extends RefreshOptions<T> {
  /** Skip the scan when the last result is younger than this. */
  maxAgeMs?: number;
  /** Only show the last result (and join a scan that is already running). */
  cacheOnly?: boolean;
}

const memory = new Map<string, Cached<unknown>>();
const running = new Map<string, Promise<unknown>>();
const generation = new Map<string, number>();
// Generation of the run that wrote what memory holds.
const written = new Map<string, number>();
// Counts changes to the user's files; a scan that overlapped one does not describe them.
let epoch = 0;
let changedAt = 0;
let pruned = false;

const clone = <T>(value: T): T =>
  value === undefined ? value : (JSON.parse(JSON.stringify(value)) as T);

function storage(): Storage | null {
  try {
    const store = globalThis.localStorage ?? null;
    if (store && !pruned) {
      pruned = true;
      for (let i = (store.length ?? 0) - 1; i >= 0; i--) {
        const key = store.key(i);
        if (key?.startsWith(FAMILY) && !key.startsWith(PREFIX)) store.removeItem(key);
      }
    }
    return store;
  } catch {
    return null;
  }
}

function read<T>(key: string): Cached<T> | null {
  const held = memory.get(key);
  if (held) return held as Cached<T>;
  try {
    const raw = storage()?.getItem(PREFIX + key);
    if (!raw) return null;
    const entry = JSON.parse(raw) as Cached<T>;
    if (typeof entry?.savedAt !== 'number' || entry.data == null) return null;
    memory.set(key, entry);
    return entry;
  } catch {
    return null;
  }
}

function write<T>(key: string, data: T, compact?: (data: T) => T): void {
  const savedAt = Date.now();
  memory.set(key, { data: clone(data), savedAt });
  const store = storage();
  if (!store) return;
  try {
    store.setItem(PREFIX + key, JSON.stringify({ data: compact ? compact(data) : data, savedAt }));
  } catch {
    // Storage full or blocked: drop the older entry so it is not shown as the latest.
    try {
      store.removeItem(PREFIX + key);
    } catch {}
  }
}

/** The last result for `key`, or null. */
export function peek<T>(key: string): Cached<T> | null {
  const entry = read<T>(key);
  return entry ? { data: clone(entry.data), savedAt: entry.savedAt } : null;
}

export function isPending(key: string): boolean {
  return running.has(key);
}

/** Runs `fetcher` and remembers its result. Callers asking at the same time share one run. */
export function refresh<T>(
  key: string,
  fetcher: () => Promise<T>,
  options: RefreshOptions<T> = {}
): Promise<T> {
  const current = running.get(key) as Promise<T> | undefined;
  if (current && !options.force) return current.then(clone);

  const mine = (generation.get(key) ?? 0) + 1;
  generation.set(key, mine);
  const superseded = () => generation.get(key) !== mine;
  // A newer run replaces this one: its answer is the one to hand out, if it has one.
  const newer = (): Promise<T> | T | undefined =>
    (running.get(key) as Promise<T> | undefined) ??
    ((written.get(key) ?? 0) > mine ? (memory.get(key)?.data as T) : undefined);

  const run: Promise<T> = new Promise<T>((resolve) => resolve(fetcher()))
    .then(
      (data) => {
        if (superseded()) return newer() ?? data;
        write(key, data, options.compact);
        written.set(key, mine);
        return data;
      },
      (error) => {
        const other = superseded() ? newer() : undefined;
        if (other !== undefined) return other;
        throw error;
      }
    )
    .finally(() => {
      if (running.get(key) === run) running.delete(key);
    });
  running.set(key, run);
  return run.then(clone);
}

/**
 * Call before and after anything that moves, deletes or restores the user's files: scans made
 * earlier no longer count as current, and nobody joins one that is still running.
 */
export function markFilesChanged(): void {
  epoch++;
  changedAt = Date.now();
  for (const key of running.keys()) generation.set(key, (generation.get(key) ?? 0) + 1);
  running.clear();
}

/**
 * Loads one view: shows the last result, then the new one. Answers that arrive after the view
 * moved on to another key are remembered but not shown.
 */
export function createLoader<T>(view: {
  show: (data: T | null, savedAt: number | null, fresh: boolean) => void;
  status: (loading: boolean, failed: boolean) => void;
}) {
  let latest = 0;
  async function load(
    key: string,
    fetcher: () => Promise<T>,
    options: LoadOptions<T> = {}
  ): Promise<void> {
    const ticket = ++latest;
    const started = epoch;
    const last = peek<T>(key);
    const recent =
      !!last &&
      !options.force &&
      !isPending(key) &&
      options.maxAgeMs !== undefined &&
      last.savedAt > changedAt &&
      Date.now() - last.savedAt < options.maxAgeMs;
    view.show(last?.data ?? null, last?.savedAt ?? null, recent);
    if (recent || (options.cacheOnly && !isPending(key))) {
      view.status(false, false);
      return;
    }
    view.status(true, false);
    try {
      const data = await refresh(key, fetcher, options);
      if (ticket !== latest) return;
      view.show(data, memory.get(key)?.savedAt ?? Date.now(), started === epoch);
      view.status(false, false);
    } catch (error) {
      if (ticket !== latest) return;
      console.error(`Failed to refresh ${key}:`, error);
      view.status(false, true);
    }
  }
  /** Ignore the answers still on their way (the view is about to ask for something else). */
  load.drop = () => void latest++;
  return load;
}

/** Keys that start with `prefix` and have a scan running. */
export function pendingKeys(prefix: string): string[] {
  return [...running.keys()].filter((key) => key.startsWith(prefix));
}

interface TreeNode {
  physical_bytes: number;
  children: TreeNode[];
}

/** Copy of a folder tree keeping at most `budget` nodes, largest folders first, level by level. */
export function trimTree<N extends TreeNode>(root: N, budget = 3000, perFolder = 80): N {
  const copy: N = { ...root, children: [] };
  const queue: [TreeNode, TreeNode][] = [[root, copy]];
  let left = budget - 1;
  for (let i = 0; i < queue.length && left > 0; i++) {
    const [from, to] = queue[i];
    const biggest = [...from.children]
      .sort((a, b) => b.physical_bytes - a.physical_bytes)
      .slice(0, Math.min(perFolder, left));
    for (const child of biggest) {
      const kept: TreeNode = { ...child, children: [] };
      to.children.push(kept);
      queue.push([child, kept]);
    }
    left -= biggest.length;
  }
  return copy;
}

/** Test helper: forget what is held in memory, as a restart would. */
export function forgetMemory(): void {
  memory.clear();
  running.clear();
  generation.clear();
  written.clear();
  changedAt = 0;
}
