import type { SigningDocument } from './types';

export type SigningDocumentPreview = 'source' | 'signed';

export function signingDocumentPreview(document: SigningDocument): SigningDocumentPreview {
  return document.has_signed_result ? 'signed' : 'source';
}

export function isSigningDocumentEditable(document: SigningDocument): boolean {
  return signingDocumentPreview(document) === 'source';
}
