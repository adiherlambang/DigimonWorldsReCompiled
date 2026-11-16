'use strict';

// Penghitung kejadian dalam jendela waktu per key (mis. IP).
// Dipakai untuk: kegagalan login (blokir brute-force) dan pembuatan akun.
class WindowCounter {
  constructor(max, windowMs) {
    this.max = max;
    this.windowMs = windowMs;
    this.hits = new Map(); // key -> number[] (timestamp)
    this.timer = setInterval(() => this.gc(), Math.max(1000, windowMs)).unref();
  }
  _prune(key, now) {
    const arr = (this.hits.get(key) || []).filter((t) => now - t < this.windowMs);
    if (arr.length) this.hits.set(key, arr); else this.hits.delete(key);
    return arr;
  }
  blocked(key, now = Date.now()) { return this._prune(key, now).length >= this.max; }
  record(key, now = Date.now()) {
    const arr = this._prune(key, now);
    arr.push(now);
    this.hits.set(key, arr);
  }
  clear(key) { this.hits.delete(key); }
  gc() { const now = Date.now(); for (const k of [...this.hits.keys()]) this._prune(k, now); }
  close() { clearInterval(this.timer); }
}

// Token bucket: rate = token/detik, burst = kapasitas.
class TokenBucket {
  constructor(rate, burst) { this.rate = rate; this.burst = burst; this.tokens = burst; this.last = Date.now(); }
  take(now = Date.now()) {
    this.tokens = Math.min(this.burst, this.tokens + ((now - this.last) / 1000) * this.rate);
    this.last = now;
    if (this.tokens >= 1) { this.tokens -= 1; return true; }
    return false;
  }
}

module.exports = { WindowCounter, TokenBucket };
