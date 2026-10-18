export function canManageSeals(systemRole: string | undefined) {
  return systemRole === 'system_admin';
}

export function signedImageDownloadName(filenameStem: string, pageCount: number) {
  return pageCount === 1
    ? `${filenameStem}-signed.png`
    : `${filenameStem}-images.zip`;
}
