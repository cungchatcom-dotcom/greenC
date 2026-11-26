// Run with: node --test ui/src/__tests__/cache.test.ts
import { test, beforeEach } from 'node:test';
import assert from 'node:assert/strict';

const disk = new Map<string, string>();
let quota = Infinity;
(globalThis as any).localStorage = {
  getItem: (k: string) => disk.get(k) ?? null,
  setItem: (k: string, v: string) => {
    if (v.length > quota) throw new Error('QuotaExceededError');
    disk.set(k, v);
  },
  removeItem: (k: string) => void disk.delete(k),
};

const { peek, refresh, isPending, createLoader, trimTree, forgetMemory } = await import('../lib/cache.ts');

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((a, b) => ((resolve = a), (reject = b)));
  return { promise, resolve, reject };
}

function recorder<T>() {
  const log: string[] = [];
  const load = createLoader<T>({
    show: (data, _at, fresh) => log.push(`show ${JSON.stringify(data)} ${fresh ? 'fresh' : 'old'}`),
    status: (loading, failed) => log.push(`status ${loading ? 'loading' : 'idle'}${failed ? ' failed' : ''}`),
  });
  return { log, load };
}

beforeEach(() => {
  disk.clear();
  forgetMemory();
  quota = Infinity;
});

test('a result is still there after the app restarts', async () => {
  await refresh('k', async () => [1, 2]);
  forgetMemory();
  assert.deepEqual(peek('k')?.data, [1, 2]);
  assert.ok(peek('k')!.savedAt > 0);
});

test('callers cannot change the cached copy by editing what they got', async () => {
  const first = await refresh('k', async () => [{ selected: true }]);
  first[0].selected = false;
  peek<{ selected: boolean }[]>('k')!.data[0].selected = false;
  assert.equal(peek<{ selected: boolean }[]>('k')!.data[0].selected, true);
});

test('two screens asking at once share one scan', async () => {
  let runs = 0;
  const d = deferred<number>();
  const a = refresh('k', () => (runs++, d.promise));
  const b = refresh('k', () => (runs++, d.promise));
  assert.equal(isPending('k'), true);
  d.resolve(7);
  assert.deepEqual(await Promise.all([a, b]), [7, 7]);
  assert.equal(runs, 1);
  assert.equal(isPending('k'), false);
});

test('a forced scan wins over an older one that finishes later', async () => {
  const before = deferred<string>();
  const after = deferred<string>();
  const a = refresh('k', () => before.promise);
  const b = refresh('k', () => after.promise, { force: true });
  after.resolve('new');
  assert.equal(await b, 'new');
  before.resolve('old');
  assert.equal(await a, 'new');
  assert.equal(peek('k')?.data, 'new');
});

test('storage that is full or broken does not lose the result for this session', async () => {
  quota = 1;
  assert.equal(await refresh('k', async () => 'big result'), 'big result');
  assert.equal(peek('k')?.data, 'big result');
  forgetMemory();
  assert.equal(peek('k'), null);
});

test('a damaged stored entry is ignored', async () => {
  await refresh('k', async () => 1);
  for (const key of disk.keys()) disk.set(key, '{not json');
  forgetMemory();
  assert.equal(peek('k'), null);
});

test('loader shows the last result first, then the new one', async () => {
  await refresh('k', async () => 'old');
  const { log, load } = recorder<string>();
  await load('k', async () => 'new');
  assert.deepEqual(log, ['show "old" old', 'status loading', 'show "new" fresh', 'status idle']);
});

test('loader keeps the last result and reports failure when the update fails', async () => {
  await refresh('k', async () => 'old');
  const { log, load } = recorder<string>();
  await load('k', async () => {
    throw new Error('boom');
  });
  assert.deepEqual(log, ['show "old" old', 'status loading', 'status idle failed']);
});

test('loader ignores a slow answer for a view the user already left', async () => {
  const slow = deferred<string>();
  const { log, load } = recorder<string>();
  const a = load('drive-c', () => slow.promise);
  await load('drive-d', async () => 'D');
  slow.resolve('C');
  await a;
  assert.equal(log.at(-2), 'show "D" fresh');
  assert.ok(!log.includes('show "C" fresh'));
  assert.equal(peek('drive-c')?.data, 'C');
});

test('loader does not rescan a result that is recent enough', async () => {
  await refresh('k', async () => 'recent');
  let runs = 0;
  const { log, load } = recorder<string>();
  await load('k', async () => (runs++, 'x'), { maxAgeMs: 60_000 });
  assert.equal(runs, 0);
  assert.deepEqual(log, ['show "recent" fresh', 'status idle']);
  await load('k', async () => (runs++, 'x'), { maxAgeMs: 60_000, force: true });
  assert.equal(runs, 1);
});

test('loader can show the last result without starting a scan', async () => {
  let runs = 0;
  const { log, load } = recorder<string>();
  await load('k', async () => (runs++, 'x'), { cacheOnly: true });
  assert.equal(runs, 0);
  assert.deepEqual(log, ['show null old', 'status idle']);
});

test('a large folder tree is stored trimmed to its biggest folders', async () => {
  const leaf = (i: number) => ({ name: `f${i}`, physical_bytes: i, children: [] as any[] });
  const root = { name: 'C', physical_bytes: 1, children: Array.from({ length: 500 }, (_, i) => leaf(i)) };
  const small = trimTree(root, 20);
  assert.equal(small.children.length, 19);
  assert.equal(small.children[0].name, 'f499');
  assert.equal(root.children.length, 500);
  const full = await refresh('tree', async () => root, { compact: (d) => trimTree(d, 20) });
  assert.equal(full.children.length, 500);
  assert.equal(peek<typeof root>('tree')!.data.children.length, 500);
  forgetMemory();
  assert.equal(peek<typeof root>('tree')!.data.children.length, 19);
});
