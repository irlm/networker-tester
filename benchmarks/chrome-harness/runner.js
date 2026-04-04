#!/usr/bin/env node
/**
 * Chrome benchmark runner for Application mode.
 * Launches headless Chrome, loads the test page, waits for results, outputs JSON.
 *
 * Usage:
 *   node runner.js --target https://endpoint:8443 --warmup 5 --measured 10 [--http-version h1|h2|h3]
 *
 * Output: JSON results to stdout (all other output goes to stderr).
 */

const puppeteer = require('puppeteer-core');
const path = require('path');

// Parse CLI args
const args = {};
for (let i = 2; i < process.argv.length; i += 2) {
  const key = process.argv[i].replace(/^--/, '');
  args[key] = process.argv[i + 1];
}

const TARGET = args.target || 'https://localhost:8443';

// SEC-007: Validate TARGET to prevent SSRF against cloud metadata / internal services
try {
  const url = new URL(TARGET);
  const host = url.hostname;
  // Block specific metadata hostnames
  const blockedHosts = ['metadata.google.internal', 'metadata.google.com'];
  if (blockedHosts.includes(host)) {
    process.stderr.write(`FATAL: TARGET hostname ${host} is blocked (cloud metadata)\n`);
    process.exit(1);
  }
  // Block IPv6 link-local
  if (host.startsWith('fe80:') || host.startsWith('[fe80:')) {
    process.stderr.write(`FATAL: TARGET in IPv6 link-local range\n`);
    process.exit(1);
  }
  // Check IPv4 — block full 169.254.0.0/16 link-local range
  const ipv4Match = host.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (ipv4Match) {
    const [, a, b] = ipv4Match.map(Number);
    if (a === 169 && b === 254) {
      process.stderr.write(`FATAL: TARGET ${host} is in link-local range (169.254.0.0/16)\n`);
      process.exit(1);
    }
  }
  // Require IP address or localhost — block arbitrary hostnames that could resolve to internal IPs
  if (!ipv4Match && host !== 'localhost') {
    process.stderr.write(`FATAL: TARGET hostname ${host} must be an IP address or localhost (hostname-based SSRF risk)\n`);
    process.exit(1);
  }
} catch (e) {
  process.stderr.write(`FATAL: Invalid TARGET URL: ${e.message}\n`);
  process.exit(1);
}

const WARMUP = parseInt(args.warmup || '5', 10);
const MEASURED = parseInt(args.measured || '10', 10);
const CONCURRENCY = parseInt(args.concurrency || '10', 10);
const HTTP_VERSION = args['http-version'] || 'h2'; // h1, h2, h3
const CONNECTION_MODE = args['connection-mode'] || 'warm'; // cold, warm
const TIMEOUT = parseInt(args.timeout || '120', 10) * 1000; // seconds -> ms
const AUTH_TOKEN = args.token || process.env.BENCH_API_TOKEN || ''; // Bearer token for auth

// Chrome determinism flags (from spec)
const CHROME_FLAGS = [
  '--disable-background-networking',
  '--disable-cache',
  '--disk-cache-size=0',
  '--disable-extensions',
  '--disable-renderer-backgrounding',
  '--disable-features=PaintHolding',
  '--disable-default-apps',
  '--no-first-run',
  '--metrics-recording-only',
  '--disable-gpu',
  '--headless=new',
  '--no-sandbox',
  '--ignore-certificate-errors', // self-signed certs
];

// HTTP version forcing
if (HTTP_VERSION === 'h1') {
  CHROME_FLAGS.push('--disable-http2');
} else if (HTTP_VERSION === 'h3') {
  // Extract host:port from target URL
  try {
    const url = new URL(TARGET);
    const hostPort = `${url.hostname}:${url.port || '443'}`;
    CHROME_FLAGS.push('--enable-quic');
    CHROME_FLAGS.push(`--origin-to-force-quic-on=${hostPort}`);
  } catch (e) {
    process.stderr.write(`Warning: Could not parse target URL for QUIC forcing: ${e.message}\n`);
  }
}

// Find Chrome binary
function findChrome() {
  const candidates = [
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/chromium-browser',
    '/usr/bin/chromium',
    '/snap/bin/chromium',
    // macOS (for local testing)
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  ];
  const fs = require('fs');
  for (const c of candidates) {
    if (fs.existsSync(c)) return c;
  }
  return null;
}

async function run() {
  const chromePath = findChrome();
  if (!chromePath) {
    throw new Error('Chrome not found. Install google-chrome or chromium.');
  }
  process.stderr.write(`Using Chrome: ${chromePath}\n`);
  process.stderr.write(`Target: ${TARGET}, HTTP: ${HTTP_VERSION}, Connection: ${CONNECTION_MODE}\n`);
  process.stderr.write(`Warmup: ${WARMUP}, Measured: ${MEASURED}, Concurrency: ${CONCURRENCY}\n`);

  const testPagePath = path.join(__dirname, 'test-page.html');
  const tokenParam = AUTH_TOKEN ? `&token=${encodeURIComponent(AUTH_TOKEN)}` : '';
  const testPageUrl = `file://${testPagePath}?target=${encodeURIComponent(TARGET)}&warmup=${WARMUP}&measured=${MEASURED}&concurrency=${CONCURRENCY}${tokenParam}`;

  if (CONNECTION_MODE === 'cold') {
    // Cold mode: run each cycle in a fresh browser instance
    return await runCold(chromePath, testPageUrl);
  } else {
    // Warm mode: single browser instance for all cycles
    return await runWarm(chromePath, testPageUrl);
  }
}

async function runWarm(chromePath, testPageUrl) {
  const browser = await puppeteer.launch({
    executablePath: chromePath,
    args: CHROME_FLAGS,
    timeout: 30000,
  });

  try {
    // Warmup Chrome process
    const warmupPage = await browser.newPage();
    await warmupPage.goto('about:blank');
    await new Promise(r => setTimeout(r, 2000));
    await warmupPage.close();

    // Run benchmark
    const page = await browser.newPage();

    // Enable CDP for protocol validation
    const cdp = await page.createCDPSession();
    await cdp.send('Network.enable');
    const protocolsSeen = new Set();
    cdp.on('Network.responseReceived', (event) => {
      if (event.response.url.startsWith(TARGET)) {
        protocolsSeen.add(event.response.protocol || 'unknown');
      }
    });

    await page.goto(testPageUrl, { waitUntil: 'domcontentloaded', timeout: TIMEOUT });

    // Wait for benchmark to complete
    const results = await page.waitForFunction(
      () => window.__benchResults,
      { timeout: TIMEOUT }
    );

    const data = await results.jsonValue();

    // Add protocol validation
    data.protocol_validation = {
      forced: HTTP_VERSION,
      observed: [...protocolsSeen],
      mismatch: false,
    };

    // Check for protocol mismatch
    const expectedProto = { h1: 'http/1.1', h2: 'h2', h3: 'h3' }[HTTP_VERSION];
    if (expectedProto && protocolsSeen.size > 0 && !protocolsSeen.has(expectedProto)) {
      data.protocol_validation.mismatch = true;
      process.stderr.write(`WARNING: Protocol mismatch! Forced ${HTTP_VERSION} but observed: ${[...protocolsSeen].join(', ')}\n`);
    }

    data.connection_mode = 'warm';
    data.http_version = HTTP_VERSION;

    await browser.close();
    return data;
  } catch (e) {
    await browser.close();
    throw e;
  }
}

async function runCold(chromePath, testPageUrl) {
  // For cold mode, we run the test page with warmup=0 in separate browser instances
  // and aggregate results ourselves.
  const coldUrl = testPageUrl.replace(/warmup=\d+/, 'warmup=0').replace(/measured=\d+/, 'measured=1');
  const allCycles = [];

  // Warmup phase (discard results)
  for (let i = 0; i < WARMUP; i++) {
    process.stderr.write(`Cold warmup ${i + 1}/${WARMUP}...\n`);
    const browser = await puppeteer.launch({
      executablePath: chromePath,
      args: CHROME_FLAGS,
      timeout: 30000,
    });
    try {
      const page = await browser.newPage();
      await page.goto(coldUrl, { waitUntil: 'domcontentloaded', timeout: TIMEOUT });
      await page.waitForFunction(() => window.__benchResults, { timeout: TIMEOUT });
    } catch (e) {
      process.stderr.write(`Cold warmup cycle ${i + 1} failed: ${e.message}\n`);
    }
    await browser.close();
  }

  // Measured phase
  const protocolsSeen = new Set();
  for (let i = 0; i < MEASURED; i++) {
    process.stderr.write(`Cold measured ${i + 1}/${MEASURED}...\n`);
    const browser = await puppeteer.launch({
      executablePath: chromePath,
      args: CHROME_FLAGS,
      timeout: 30000,
    });
    try {
      const page = await browser.newPage();
      const cdp = await page.createCDPSession();
      await cdp.send('Network.enable');
      cdp.on('Network.responseReceived', (event) => {
        if (event.response.url.includes(new URL(TARGET).hostname)) {
          protocolsSeen.add(event.response.protocol || 'unknown');
        }
      });

      await page.goto(coldUrl, { waitUntil: 'domcontentloaded', timeout: TIMEOUT });
      const results = await page.waitForFunction(() => window.__benchResults, { timeout: TIMEOUT });
      const data = await results.jsonValue();
      if (data.measured && data.measured[0]) {
        allCycles.push(data.measured[0]);
      }
    } catch (e) {
      process.stderr.write(`Cold measured cycle ${i + 1} failed: ${e.message}\n`);
      allCycles.push({ cycle_duration_ms: 0, request_count: 0, error_count: CONCURRENCY, requests: [], throughput_rps: 0 });
    }
    await browser.close();
  }

  // Aggregate cold results
  const durations = allCycles.map(c => c.cycle_duration_ms).filter(d => d > 0);
  durations.sort((a, b) => a - b);

  const avg = (arr) => arr.length > 0 ? arr.reduce((s, v) => s + v, 0) / arr.length : 0;
  const pct = (arr, p) => arr.length > 0 ? arr[Math.floor(arr.length * p)] : 0;

  const result = {
    warmup: [],
    measured: allCycles,
    meta: {
      target: TARGET,
      warmup_cycles: WARMUP,
      measured_cycles: MEASURED,
      concurrency: CONCURRENCY,
      started_at: new Date().toISOString(),
      finished_at: new Date().toISOString(),
    },
    summary: {
      cycle_count: durations.length,
      total_requests: allCycles.reduce((s, c) => s + c.request_count, 0),
      total_errors: allCycles.reduce((s, c) => s + c.error_count, 0),
      cycle_duration: {
        mean: avg(durations),
        p50: pct(durations, 0.5),
        p95: pct(durations, 0.95),
        p99: pct(durations, 0.99),
        min: durations[0] || 0,
        max: durations[durations.length - 1] || 0,
      },
      avg_throughput_rps: avg(allCycles.map(c => c.throughput_rps)),
    },
    connection_mode: 'cold',
    http_version: HTTP_VERSION,
    protocol_validation: {
      forced: HTTP_VERSION,
      observed: [...protocolsSeen],
      mismatch: false,
    },
  };

  const expectedProto = { h1: 'http/1.1', h2: 'h2', h3: 'h3' }[HTTP_VERSION];
  if (expectedProto && protocolsSeen.size > 0 && !protocolsSeen.has(expectedProto)) {
    result.protocol_validation.mismatch = true;
  }

  return result;
}

// Main
run()
  .then(results => {
    process.stdout.write(JSON.stringify(results));
    process.exit(0);
  })
  .catch(e => {
    process.stderr.write(`FATAL: ${e.message}\n${e.stack}\n`);
    process.stdout.write(JSON.stringify({ error: e.message }));
    process.exit(1);
  });
