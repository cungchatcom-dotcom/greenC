import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const PORT = Number(process.env.PORT || 3010);
const HOST = process.env.HOST || '127.0.0.1';
const DIST_DIR = path.resolve(process.env.DIST_DIR || path.join(__dirname, 'dist'));
const ACCOUNT_API_BASE = (process.env.ACCOUNT_API_BASE || 'https://account.cungchat.com').replace(/\/+$/, '');
const INSTALLER_FILENAME = process.env.INSTALLER_FILENAME || 'GreenC-Setup-2.0.0.exe';
const INSTALLER_PATH = path.resolve(
  process.env.INSTALLER_PATH ||
    (fs.existsSync(path.join(__dirname, 'releases', INSTALLER_FILENAME))
      ? path.join(__dirname, 'releases', INSTALLER_FILENAME)
      : path.join(__dirname, '..', 'dist', INSTALLER_FILENAME))
);

const TICKET_TTL_MS = 60_000;
const tickets = new Map();

// Periodically purge expired tickets
setInterval(() => {
  const now = Date.now();
  for (const [key, entry] of tickets.entries()) {
    if (entry.expiresAt <= now) {
      tickets.delete(key);
    }
  }
}, 30_000).unref();

const MIME_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.ico': 'image/x-icon',
  '.json': 'application/json; charset=utf-8',
  '.woff2': 'font/woff2',
};

function setSecurityHeaders(res) {
  res.setHeader('X-Content-Type-Options', 'nosniff');
  res.setHeader('X-Frame-Options', 'DENY');
  res.setHeader('Referrer-Policy', 'strict-origin-when-cross-origin');
  res.setHeader('Permissions-Policy', 'camera=(), microphone=(), geolocation=()');
}

function sendJson(res, status, payload) {
  setSecurityHeaders(res);
  res.writeHead(status, {
    'Content-Type': 'application/json; charset=utf-8',
    'Cache-Control': 'no-store',
  });
  res.end(JSON.stringify(payload));
}

async function verifyCungChatToken(authHeader) {
  if (!authHeader || !authHeader.startsWith('Bearer ')) {
    return null;
  }
  const token = authHeader.slice('Bearer '.length).trim();
  if (!token || token.length > 4096) {
    return null;
  }
  try {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 8000);
    const resp = await fetch(`${ACCOUNT_API_BASE}/users/me`, {
      method: 'GET',
      headers: { Authorization: `Bearer ${token}` },
      signal: controller.signal,
    });
    clearTimeout(timer);
    if (!resp.ok) return null;
    const user = await resp.json();
    if (!user || !user.email) return null;
    return user;
  } catch (err) {
    console.error('[auth] verify failed:', err instanceof Error ? err.message : err);
    return null;
  }
}

function serveStaticFile(req, res, filePath) {
  const ext = path.extname(filePath).toLowerCase();
  const contentType = MIME_TYPES[ext] || 'application/octet-stream';
  const isImmutable = filePath.includes(`${path.sep}assets${path.sep}`);
  let cacheControl = 'public, max-age=3600';
  if (isImmutable) {
    cacheControl = 'public, max-age=31536000, immutable';
  } else if (ext === '.html') {
    cacheControl = 'no-cache';
  }

  setSecurityHeaders(res);
  res.writeHead(200, {
    'Content-Type': contentType,
    'Cache-Control': cacheControl,
  });

  if (req.method === 'HEAD') {
    res.end();
    return;
  }

  const stream = fs.createReadStream(filePath);
  stream.on('error', () => {
    if (!res.headersSent) res.writeHead(500);
    res.end();
  });
  stream.pipe(res);
}

const server = http.createServer(async (req, res) => {
  try {
    const reqUrl = new URL(req.url || '/', `http://${req.headers.host || 'localhost'}`);
    const pathname = reqUrl.pathname;

    // Health endpoint
    if (req.method === 'GET' && pathname === '/api/health') {
      const exists = fs.existsSync(INSTALLER_PATH);
      const size = exists ? fs.statSync(INSTALLER_PATH).size : 0;
      return sendJson(res, 200, {
        ok: true,
        service: 'greenc-landing',
        version: '2.0.0',
        installerAvailable: exists,
        installerSize: size,
      });
    }

    // Issue a single-use download ticket only after verifying cungchat-account JWT
    if (req.method === 'POST' && pathname === '/api/download-ticket') {
      const user = await verifyCungChatToken(req.headers.authorization);
      if (!user) {
        return sendJson(res, 401, { error: 'Vui lòng đăng nhập tài khoản Cùng Chat để tải GreenC.' });
      }
      if (!fs.existsSync(INSTALLER_PATH)) {
        return sendJson(res, 503, { error: 'Bộ cài đặt hiện chưa sẵn sàng trên máy chủ.' });
      }
      const stat = fs.statSync(INSTALLER_PATH);
      const ticket = crypto.randomBytes(24).toString('hex');
      tickets.set(ticket, {
        email: user.email,
        expiresAt: Date.now() + TICKET_TTL_MS,
      });
      return sendJson(res, 200, {
        ticket,
        filename: INSTALLER_FILENAME,
        size: stat.size,
        expiresInSeconds: TICKET_TTL_MS / 1000,
      });
    }

    // Stream installer using valid single-use ticket
    if (req.method === 'GET' && pathname === '/api/download') {
      const ticket = reqUrl.searchParams.get('ticket');
      if (!ticket || !tickets.has(ticket)) {
        return sendJson(res, 401, { error: 'Liên kết tải không hợp lệ hoặc đã hết hạn.' });
      }
      const entry = tickets.get(ticket);
      tickets.delete(ticket); // single-use
      if (entry.expiresAt <= Date.now()) {
        return sendJson(res, 401, { error: 'Liên kết tải đã hết hạn. Vui lòng bấm tải lại.' });
      }
      if (!fs.existsSync(INSTALLER_PATH)) {
        return sendJson(res, 503, { error: 'Không tìm thấy file cài đặt.' });
      }

      const stat = fs.statSync(INSTALLER_PATH);
      setSecurityHeaders(res);
      res.writeHead(200, {
        'Content-Type': 'application/vnd.microsoft.portable-executable',
        'Content-Length': stat.size,
        'Content-Disposition': `attachment; filename="${INSTALLER_FILENAME}"`,
        'Cache-Control': 'no-store',
      });
      console.log(`[download] user=${entry.email} file=${INSTALLER_FILENAME} size=${stat.size}`);
      const stream = fs.createReadStream(INSTALLER_PATH);
      stream.on('error', () => {
        if (!res.headersSent) res.writeHead(500);
        res.end();
      });
      stream.pipe(res);
      return;
    }

    // Static assets & SPA callback route
    if (req.method === 'GET' || req.method === 'HEAD') {
      let decodedPath;
      try {
        decodedPath = decodeURIComponent(pathname);
      } catch {
        res.writeHead(400);
        res.end('Bad Request');
        return;
      }

      const safeRelPath = path.normalize(decodedPath).replace(/^(\.\.[/\\])+/, '');
      let candidate = path.join(DIST_DIR, safeRelPath === path.sep ? 'index.html' : safeRelPath);

      // Ensure resolved path stays strictly inside DIST_DIR
      const resolvedDist = path.resolve(DIST_DIR);
      const resolvedCandidate = path.resolve(candidate);
      const relative = path.relative(resolvedDist, resolvedCandidate);
      if (relative.startsWith('..') || path.isAbsolute(relative)) {
        res.writeHead(403);
        res.end('Forbidden');
        return;
      }

      if (fs.existsSync(resolvedCandidate) && fs.statSync(resolvedCandidate).isFile()) {
        return serveStaticFile(req, res, resolvedCandidate);
      }

      // Serve index.html for root, /terms, and /auth/callback
      const indexHtml = path.join(resolvedDist, 'index.html');
      if ((pathname === '/' || pathname === '/terms' || pathname === '/auth/callback') && fs.existsSync(indexHtml)) {
        return serveStaticFile(req, res, indexHtml);
      }

      res.writeHead(404, { 'Content-Type': 'text/plain; charset=utf-8' });
      res.end('Not Found');
      return;
    }

    res.writeHead(405);
    res.end('Method Not Allowed');
  } catch (err) {
    console.error('[server] unhandled error:', err);
    if (!res.headersSent) {
      sendJson(res, 500, { error: 'Internal Server Error' });
    }
  }
});

server.listen(PORT, HOST, () => {
  console.log(`[greenc-landing] listening on http://${HOST}:${PORT}`);
  console.log(`[greenc-landing] dist=${DIST_DIR}`);
  console.log(`[greenc-landing] installer=${INSTALLER_PATH} (exists=${fs.existsSync(INSTALLER_PATH)})`);
  console.log(`[greenc-landing] account=${ACCOUNT_API_BASE}`);
});
