// Run with: node --experimental-strip-types --test ui/src/__tests__/screen-timer.test.ts
import { test } from 'node:test';
import assert from 'node:assert/strict';

const { createScreenTimer } = await import('../lib/screen-timer.ts');

test('reports elapsed ms and cached=true on first shown(false), ignores subsequent calls', () => {
  let clock = 100;
  const events: Array<{ ms: number; cached: boolean }> = [];
  const timer = createScreenTimer(
    (ms, cached) => events.push({ ms, cached }),
    () => clock
  );

  clock = 145.6;
  timer.shown(false);
  clock = 500;
  timer.shown(true);
  timer.shown(false);

  assert.deepEqual(events, [{ ms: 46, cached: true }]);
});

test('reports cached=false when first shown(true) is fresh', () => {
  let clock = 250;
  const events: Array<{ ms: number; cached: boolean }> = [];
  const timer = createScreenTimer(
    (ms, cached) => events.push({ ms, cached }),
    () => clock
  );

  clock = 1_250;
  timer.shown(true);
  clock = 2_000;
  timer.shown(false);

  assert.deepEqual(events, [{ ms: 1_000, cached: false }]);
});

test('clamps negative clock deltas to 0', () => {
  let clock = 500;
  const events: Array<{ ms: number; cached: boolean }> = [];
  const timer = createScreenTimer(
    (ms, cached) => events.push({ ms, cached }),
    () => clock
  );

  clock = 400;
  timer.shown(true);

  assert.deepEqual(events, [{ ms: 0, cached: false }]);
});
