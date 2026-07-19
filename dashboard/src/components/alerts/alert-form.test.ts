// Rule-builder + channel-editor validation, mirroring the backend contract
// (AlertsEndpoints / AlertRuleLogic — docs/alerting.md).

import { describe, expect, it } from 'vitest';
import {
  channelConfigFromForm,
  formatCondition,
  formatThreshold,
  MAX_WINDOW_RUNS,
  parseRecipients,
  SECRET_MASK,
  validateChannelForm,
  validateRuleForm,
  type ChannelFormValues,
  type RuleFormValues,
} from './alert-form';

const validRule: RuleFormValues = {
  metric: 'p95_ms',
  comparator: 'gt',
  threshold: '500',
  windowRuns: '3',
  channelId: 'ch-1',
  testConfigId: '',
};

describe('validateRuleForm', () => {
  it('accepts a valid rule', () => {
    expect(validateRuleForm(validRule)).toBeNull();
  });

  it('rejects a missing threshold', () => {
    expect(validateRuleForm({ ...validRule, threshold: '' })).toMatch(/finite number/i);
  });

  it('rejects a non-numeric threshold', () => {
    expect(validateRuleForm({ ...validRule, threshold: 'abc' })).toMatch(/finite number/i);
  });

  it('rejects out-of-range rate thresholds (ratios are 0..1)', () => {
    expect(validateRuleForm({ ...validRule, metric: 'error_rate', threshold: '5' })).toMatch(/between 0 and 1/i);
    expect(validateRuleForm({ ...validRule, metric: 'success_rate', threshold: '-0.1' })).toMatch(/between 0 and 1/i);
    expect(validateRuleForm({ ...validRule, metric: 'error_rate', threshold: '0.05' })).toBeNull();
  });

  it('allows large ms thresholds (only rates are ratio-bounded)', () => {
    expect(validateRuleForm({ ...validRule, metric: 'mean_ms', threshold: '30000' })).toBeNull();
  });

  it('rejects window outside 1..50 or non-integer', () => {
    expect(validateRuleForm({ ...validRule, windowRuns: '0' })).toMatch(/between 1 and 50/i);
    expect(validateRuleForm({ ...validRule, windowRuns: String(MAX_WINDOW_RUNS + 1) })).toMatch(/between 1 and 50/i);
    expect(validateRuleForm({ ...validRule, windowRuns: '2.5' })).toMatch(/between 1 and 50/i);
    expect(validateRuleForm({ ...validRule, windowRuns: '' })).toMatch(/between 1 and 50/i);
    expect(validateRuleForm({ ...validRule, windowRuns: String(MAX_WINDOW_RUNS) })).toBeNull();
  });

  it('requires a channel', () => {
    expect(validateRuleForm({ ...validRule, channelId: '' })).toMatch(/channel/i);
  });
});

const validWebhook: ChannelFormValues = {
  kind: 'webhook',
  name: 'ops hook',
  url: 'https://hooks.example.com/networker',
  secret: '',
  to: '',
};

const validEmail: ChannelFormValues = {
  kind: 'email',
  name: 'on-call',
  url: '',
  secret: '',
  to: 'sre@example.com, oncall@example.com',
};

describe('validateChannelForm', () => {
  it('accepts valid webhook and email channels', () => {
    expect(validateChannelForm(validWebhook)).toBeNull();
    expect(validateChannelForm(validEmail)).toBeNull();
  });

  it('requires a name', () => {
    expect(validateChannelForm({ ...validWebhook, name: '  ' })).toMatch(/name/i);
  });

  it('rejects relative or non-http(s) webhook URLs', () => {
    expect(validateChannelForm({ ...validWebhook, url: '/hooks/networker' })).toMatch(/absolute http/i);
    expect(validateChannelForm({ ...validWebhook, url: 'ftp://example.com/x' })).toMatch(/absolute http/i);
    expect(validateChannelForm({ ...validWebhook, url: '' })).toMatch(/absolute http/i);
  });

  it('requires at least one valid email recipient', () => {
    expect(validateChannelForm({ ...validEmail, to: '' })).toMatch(/at least one/i);
    expect(validateChannelForm({ ...validEmail, to: 'not-an-address' })).toMatch(/not a valid email/i);
  });
});

describe('channelConfigFromForm', () => {
  it('omits an empty webhook secret (removes stored signing)', () => {
    expect(channelConfigFromForm(validWebhook)).toEqual({ url: 'https://hooks.example.com/networker' });
  });

  it('round-trips the mask so the stored secret is kept', () => {
    expect(channelConfigFromForm({ ...validWebhook, secret: SECRET_MASK })).toEqual({
      url: 'https://hooks.example.com/networker',
      secret: SECRET_MASK,
    });
  });

  it('splits recipients on commas, whitespace, and newlines', () => {
    expect(parseRecipients('a@x.com, b@x.com\nc@x.com;  d@x.com')).toEqual([
      'a@x.com',
      'b@x.com',
      'c@x.com',
      'd@x.com',
    ]);
    expect(channelConfigFromForm(validEmail)).toEqual({ to: ['sre@example.com', 'oncall@example.com'] });
  });
});

describe('condition formatting', () => {
  it('appends ms only to latency metrics', () => {
    expect(formatThreshold('p95_ms', 500)).toBe('500ms');
    expect(formatThreshold('error_rate', 0.05)).toBe('0.05');
  });

  it('renders the terminal-style condition', () => {
    expect(formatCondition('p95_ms', 'gt', 500)).toBe('p95_ms > 500ms');
    expect(formatCondition('success_rate', 'lt', 0.99)).toBe('success_rate < 0.99');
  });
});
