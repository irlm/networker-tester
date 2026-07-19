/**
 * Strip ANSI/VT100 escape sequences from a string.
 *
 * Tester log lines (SGR color codes like `\x1b[2m`, `\x1b[32m`) end up stored
 * in `error_message` by the agent and were rendered verbatim in the UI
 * (design audit F8). Until the control plane strips them at the source, this
 * is the belt-and-braces display-side guard.
 *
 * Covers, in order: OSC sequences (ESC ] ... BEL/ST — terminal title etc.),
 * CSI sequences (ESC [ params intermediates final-byte — SGR colors, cursor
 * movement, erase), and two-byte ESC codes.
 */
// eslint-disable-next-line no-control-regex
const ANSI_PATTERN = new RegExp(
  [
    '\\u001B\\][^\\u0007\\u001B]*(?:\\u0007|\\u001B\\\\)',
    '\\u001B\\[[0-9;:?]*[ -/]*[@-~]',
    '\\u001B[@-Z\\\\-_]',
    '\\u009B[0-9;:?]*[ -/]*[@-~]',
  ].join('|'),
  'g',
);

export function stripAnsi(input: string): string {
  return input.replace(ANSI_PATTERN, '');
}
