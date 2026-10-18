import assert from 'node:assert/strict';
import test from 'node:test';
import { canManageSeals, signedImageDownloadName } from '../src/seal-governance.ts';

test('only system administrators receive seal management controls', () => {
  assert.equal(canManageSeals('system_admin'), true);
  assert.equal(canManageSeals('member'), false);
  assert.equal(canManageSeals(undefined), false);
});

test('single-page image exports download PNG while multi-page exports use ZIP', () => {
  assert.equal(signedImageDownloadName('contract', 1), 'contract-signed.png');
  assert.equal(signedImageDownloadName('contract', 2), 'contract-images.zip');
});
