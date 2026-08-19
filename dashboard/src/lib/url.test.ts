import { describe, expect, it } from 'vitest';
import { isAbsoluteHttpUrl, normalizeHttpUrl } from './url';

describe('shared HTTP URL validation', () => {
  it('normalizes configured URLs and trims one trailing slash', () => {
    expect(normalizeHttpUrl(' https://demo.example.test/ ')).toBe('https://demo.example.test');
    expect(isAbsoluteHttpUrl('http://localhost:8080')).toBe(true);
  });

  it('rejects missing, relative, and non-http URLs', () => {
    expect(normalizeHttpUrl(undefined)).toBeUndefined();
    expect(isAbsoluteHttpUrl('/laghound/echo')).toBe(false);
    expect(isAbsoluteHttpUrl('ftp://demo.example.test')).toBe(false);
  });
});
