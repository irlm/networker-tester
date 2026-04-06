/**
 * Only calls setState if the JSON representation of the new value
 * differs from the current one. Prevents unnecessary re-renders when
 * polling returns identical data.
 *
 * Uses a fast fingerprint comparison (JSON.stringify) instead of deep
 * equality to keep it simple and allocation-light for small datasets.
 */
export function stableSet<T>(
  setter: React.Dispatch<React.SetStateAction<T>>,
  newValue: T,
  prevRef: React.MutableRefObject<string>,
): boolean {
  const fingerprint = JSON.stringify(newValue);
  if (fingerprint === prevRef.current) return false;
  prevRef.current = fingerprint;
  setter(newValue);
  return true;
}
