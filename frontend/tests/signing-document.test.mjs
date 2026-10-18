import assert from 'node:assert/strict';
import test from 'node:test';
import { isSigningDocumentEditable, signingDocumentPreview } from '../src/signing-document.ts';

function signingDocument(hasSignedResult) {
  return {
    id: 'document-1',
    original_filename: 'contract.pdf',
    source_format: 'pdf',
    source_size_bytes: 1024,
    page_count: 1,
    has_signed_result: hasSignedResult,
    signed_at: hasSignedResult ? '2026-07-27T08:00:00Z' : null,
    created_at: '2026-07-27T07:00:00Z',
    updated_at: '2026-07-27T08:00:00Z',
  };
}

test('finalized documents use the merged PDF and are no longer editable', () => {
  const document = signingDocument(true);
  assert.equal(signingDocumentPreview(document), 'signed');
  assert.equal(isSigningDocumentEditable(document), false);
});

test('pending documents use the source PDF and remain editable', () => {
  const document = signingDocument(false);
  assert.equal(signingDocumentPreview(document), 'source');
  assert.equal(isSigningDocumentEditable(document), true);
});
