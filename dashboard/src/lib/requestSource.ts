import type { RequestSource } from '../stores/apiLogStore';

/** Global flag: set to 'poll' by usePolling before each tick, reset to 'user' after. */
let current: RequestSource = 'user';

export function setRequestSource(source: RequestSource) {
  current = source;
}

export function getRequestSource(): RequestSource {
  return current;
}
