'use strict';

const { isMainThread } = require('node:worker_threads');

const SELF_REQUIRE = /\s*--require\s+(?:"(?:[^"\\]|\\.)*cursor-tty\.cjs"|[^\s"]*cursor-tty\.cjs)/g;
const ALT_ON = /\x1b\[\?(?:1049|1047|47)h/g;
const ALT_OFF = /\x1b\[\?(?:1049|1047|47)l/g;
const NON_LAYOUT = /^(?:\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[?>=<][0-9;]*[hlmu])+$/;
const KITTY_START = '\x1b[>1u';
const FULL_CLEAR = /^\x1b\[2J(?:\x1b\[3J\x1b\[H|\x1b\[0f)/;
const ERASE_PREFIX = /^(?:\x1b\[2K\x1b\[1A)*\x1b\[2K\x1b\[G/;
const ERASE_LINE_UP = '\x1b[2K\x1b[1A';
const ERASE_LINE_HOME = '\x1b[2K\x1b[G';
const SYNC_START = '\x1b[?2026h';
const SYNC_END = '\x1b[?2026l';
const RESET_SGR = '\x1b[0m';

function stripNodeOptions() {
  const current = process.env.NODE_OPTIONS;
  if (!current) return;
  const cleaned = current.replace(SELF_REQUIRE, '').trim();
  if (cleaned) process.env.NODE_OPTIONS = cleaned;
  else delete process.env.NODE_OPTIONS;
}

function lastMatchEnd(re, text) {
  re.lastIndex = 0;
  let end = -1;
  let m = re.exec(text);
  while (m !== null) {
    end = m.index + m[0].length;
    m = re.exec(text);
  }
  return end;
}

function eraseLines(count) {
  if (count <= 0) return '';
  return ERASE_LINE_UP.repeat(count - 1) + ERASE_LINE_HOME;
}

function finishSkipped(encoding, cb) {
  const done = typeof encoding === 'function' ? encoding : typeof cb === 'function' ? cb : null;
  if (done) process.nextTick(done);
  return true;
}

function forward(origWrite, target, out, argCount, encoding, cb) {
  return argCount <= 1 ? origWrite.call(target, out) : origWrite.call(target, out, encoding, cb);
}

function validFullState(lines, rows) {
  return rows > 0 && lines.length > rows && lines[lines.length - 1] === '';
}

function install(stdout) {
  const origWrite = stdout.write;
  let prev = null;
  let altScreen = false;
  let lastCols = stdout.columns || 80;
  let lastRows = stdout.rows || 24;

  if (typeof stdout.on === 'function') {
    stdout.on('resize', () => {
      prev = null;
      lastCols = stdout.columns || 80;
      lastRows = stdout.rows || 24;
    });
  }

  stdout.write = function write(chunk, encoding, cb) {
    if (typeof chunk !== 'string') {
      prev = null;
      return origWrite.apply(this, arguments);
    }

    const cols = stdout.columns || 80;
    const rows = stdout.rows || 24;
    if (cols !== lastCols || rows !== lastRows) {
      prev = null;
      lastCols = cols;
      lastRows = rows;
    }

    const onEnd = lastMatchEnd(ALT_ON, chunk);
    const offEnd = lastMatchEnd(ALT_OFF, chunk);
    if (onEnd >= 0 || offEnd >= 0) {
      altScreen = onEnd > offEnd;
      prev = null;
      return origWrite.apply(this, arguments);
    }
    if (altScreen) {
      return origWrite.apply(this, arguments);
    }

    if (NON_LAYOUT.test(chunk)) {
      if (chunk.includes(KITTY_START)) prev = [''];
      return origWrite.apply(this, arguments);
    }

    const clearMatch = FULL_CLEAR.exec(chunk);
    if (clearMatch !== null && chunk.length > clearMatch[0].length) {
      const next = chunk.slice(clearMatch[0].length).split('\n');
      if (
        prev !== null &&
        validFullState(prev, rows) &&
        validFullState(next, rows) &&
        next.length >= prev.length &&
        prev[0] === next[0]
      ) {
        const sameLength = next.length === prev.length;
        const base = prev.length - rows;
        const oldView = prev.slice(base);
        const newTail = next.slice(base);
        let diff = -1;
        for (let i = 0; i < rows; i++) {
          if (oldView[i] !== newTail[i]) {
            diff = i;
            break;
          }
        }
        prev = next;
        if (diff === -1 && sameLength) {
          return finishSkipped(encoding, cb);
        }
        const from = diff === -1 ? rows - 1 : diff;
        const out = SYNC_START + RESET_SGR + eraseLines(rows - from) + newTail.slice(from).join('\n') + SYNC_END;
        return forward(origWrite, this, out, arguments.length, encoding, cb);
      }
      prev = validFullState(next, rows) ? next : null;
      return forward(origWrite, this, SYNC_START + chunk + SYNC_END, arguments.length, encoding, cb);
    }

    const eraseMatch = ERASE_PREFIX.exec(chunk);
    if (eraseMatch !== null) {
      if (prev !== null) {
        const erased = (eraseMatch[0].length - ERASE_LINE_HOME.length) / ERASE_LINE_UP.length + 1;
        const tail = chunk.slice(eraseMatch[0].length).split('\n');
        if (
          erased <= prev.length &&
          erased <= rows &&
          tail.length >= erased &&
          tail[tail.length - 1] === ''
        ) {
          prev = prev.slice(0, prev.length - erased).concat(tail);
        } else {
          prev = null;
        }
      }
      return origWrite.apply(this, arguments);
    }

    prev = null;
    return origWrite.apply(this, arguments);
  };
}

if (isMainThread && process.stdout && process.stdout.isTTY && process.argv.includes('--plugin-dir')) {
  stripNodeOptions();
  install(process.stdout);
}
