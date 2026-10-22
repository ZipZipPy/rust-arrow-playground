/** A message for the UI from anything a failed request or decode can throw. */
export function errorMessage(error: unknown): string {
  if (error instanceof TypeError) return `Network error: ${error.message}. Is the server running and reachable (CORS)?`;
  if (error instanceof RangeError) {
    return `The browser could not hold the decoded response (${error.message}). Try fewer rows or columns.`;
  }
  return error instanceof Error ? error.message : String(error);
}
