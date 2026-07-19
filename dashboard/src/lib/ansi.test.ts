import { describe, expect, it } from 'vitest';
import { stripAnsi } from './ansi';

const ESC = '';
const BEL = '';

describe('stripAnsi', () => {
  it('passes plain text through unchanged', () => {
    expect(stripAnsi('connection refused (os error 111)')).toBe('connection refused (os error 111)');
  });

  it('leaves bracketed text without escape bytes alone', () => {
    expect(stripAnsi('[tester] probe failed')).toBe('[tester] probe failed');
  });

  it('strips SGR color/style codes', () => {
    expect(stripAnsi(`${ESC}[32m INFO ${ESC}[0m ready`)).toBe(' INFO  ready');
    expect(stripAnsi(`${ESC}[2m2026-07-14T01:22:24.974248Z${ESC}[0m`)).toBe('2026-07-14T01:22:24.974248Z');
  });

  it('strips the exact tester log-line pattern from the audit (F8)', () => {
    const raw = `[tester] ${ESC}[2m2026-07-14T01:22:24.974248Z${ESC}[0m ${ESC}[32m INFO${ESC}[0m ${ESC}[2mnetworker_tester${ESC}[0m probe failed`;
    expect(stripAnsi(raw)).toBe('[tester] 2026-07-14T01:22:24.974248Z  INFO networker_tester probe failed');
  });

  it('strips multi-parameter and cursor/erase sequences', () => {
    expect(stripAnsi(`${ESC}[1;31mFAIL${ESC}[0m ${ESC}[2K${ESC}[1Adone`)).toBe('FAIL done');
  });

  it('strips OSC sequences (terminal title set)', () => {
    expect(stripAnsi(`${ESC}]0;tester${BEL}run complete`)).toBe('run complete');
  });

  it('handles empty strings', () => {
    expect(stripAnsi('')).toBe('');
  });
});
