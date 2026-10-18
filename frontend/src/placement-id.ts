let placementIdSequence = 0;

export function nextPlacementId() {
  placementIdSequence += 1;
  return `placement-${Date.now().toString(36)}-${placementIdSequence.toString(36)}`;
}
