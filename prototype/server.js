import http from 'node:http';
import os from 'node:os';
import { readFile } from 'node:fs/promises';

const PORT = Number(process.env.PORT) || 3000;
const HISTORY_LIMIT = 200;
const MAX_TEXT = 2000;
const MAX_NAME = 32;

/** @type {Map<http.ServerResponse, string>} open event streams -> display name */
const clients = new Map();
const history = [];

const cleanName = (s) => String(s ?? '').trim().slice(0, MAX_NAME);
const onlineUsers = () => [...new Set(clients.values())].sort((a, b) => a.localeCompare(b));

function send(res, event, data) {
  res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
}

function broadcast(event, data) {
  for (const res of clients.keys()) send(res, event, data);
}

function post(msg) {
  msg.ts = Date.now();
  history.push(msg);
  if (history.length > HISTORY_LIMIT) history.shift();
  broadcast('message', msg);
}

function readBody(req, limit = 16_384) {
  return new Promise((resolve, reject) => {
    let body = '';
    req.on('data', (chunk) => {
      body += chunk;
      if (body.length > limit) reject(new Error('too large'));
    });
    req.on('end', () => resolve(body));
    req.on('error', reject);
  });
}

function openStream(req, res, name) {
  res.writeHead(200, {
    'content-type': 'text/event-stream',
    'cache-control': 'no-cache',
    connection: 'keep-alive',
  });
  send(res, 'history', history);

  const isNew = !onlineUsers().includes(name); // same name in a second tab isn't a new join
  clients.set(res, name);
  if (isNew) post({ system: true, text: `${name} joined` });
  broadcast('users', onlineUsers());

  const ping = setInterval(() => res.write(': ping\n\n'), 25_000);
  req.on('close', () => {
    clearInterval(ping);
    clients.delete(res);
    if (!onlineUsers().includes(name)) post({ system: true, text: `${name} left` });
    broadcast('users', onlineUsers());
  });
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, 'http://localhost');

  try {
    if (req.method === 'GET' && url.pathname === '/') {
      const page = await readFile(new URL('./public/index.html', import.meta.url));
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      return res.end(page);
    }

    if (req.method === 'GET' && url.pathname === '/events') {
      const name = cleanName(url.searchParams.get('name'));
      if (!name) {
        res.writeHead(400);
        return res.end('name required');
      }
      return openStream(req, res, name);
    }

    if (req.method === 'POST' && url.pathname === '/send') {
      const { name, text } = JSON.parse(await readBody(req));
      const n = cleanName(name);
      const t = String(text ?? '').trim().slice(0, MAX_TEXT);
      if (!n || !t) {
        res.writeHead(400);
        return res.end('name and text required');
      }
      post({ name: n, text: t });
      res.writeHead(204);
      return res.end();
    }

    res.writeHead(404);
    res.end('not found');
  } catch (err) {
    res.writeHead(400);
    res.end(String(err.message));
  }
});

server.listen(PORT, '0.0.0.0', () => {
  console.log(`xtalk running. Open one of these on any device on your network:`);
  console.log(`  http://localhost:${PORT}`);
  for (const addrs of Object.values(os.networkInterfaces())) {
    for (const a of addrs ?? []) {
      if (a.family === 'IPv4' && !a.internal) console.log(`  http://${a.address}:${PORT}`);
    }
  }
});
