import assert from 'node:assert/strict';
import test from 'node:test';
import { nextPlacementId } from '../src/placement-id.ts';

test('creates unique placement IDs without secure-context crypto APIs', () => {
  const cryptoDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'crypto');
  Object.defineProperty(globalThis, 'crypto', {
    configurable: true,
    value: {},
  });

  try {
    const first = nextPlacementId();
    const second = nextPlacementId();
    assert.match(first, /^placement-[a-z0-9]+-[a-z0-9]+$/);
    assert.notEqual(first, second);
  } finally {
    if (cryptoDescriptor) Object.defineProperty(globalThis, 'crypto', cryptoDescriptor);
    else delete globalThis.crypto;
  }
});
