// mdio-mcp-bridge.mjs — stdio ↔ http 桥 + gateway 自启(零依赖,不改 dsh)
//
// ⚠️ 已退役：DSH 的 mcp-mdio 已改为 streamable-http 直连
//    http://127.0.0.1:8012/mcp，本脚本不再被 cordis.patch.yml 引用。
//    退役原因：它原本是为「DSH 不支持 2026-07-28 无状态协议」做的转接；
//    mdio 侧补上 resultType 后（mdio_py/gateway/api/mcp.py），双方都原生说新协议，
//    这层转接没有存在意义了。网关生命周期改由 scripts/mdio-gateway-up.sh 承担。
//    本文件保留备查；要回退，只需把 profile 的 mcp-mdio 改回 stdio + 本脚本。
//
// 原职责:
//   1) 检查 MDIO gateway 是否在跑,没跑就拉起(带 MDIO_VFS_MOUNTS)
//   2) 把 stdin 的每行 JSON-RPC 转发为 POST <URL>,结果回写 stdout(一行一条)
import { spawn } from 'node:child_process';
import readline from 'node:readline';
import { appendFileSync, statSync, writeFileSync } from 'node:fs';

// ── 诊断日志 ─────────────────────────────────────────────────────────
// 宿主的 stdio 子进程 stderr 被接到 /dev/null,mcp-client 的
// `connection attempt failed: ...` 会直接丢掉,导致无从定位。
// 本段只写日志,不改任何转发语义;不需要时删掉这一段即可。
const LOG_PATH = process.env.MDIO_BRIDGE_LOG || new URL('../.tmp/mdio-bridge.log', import.meta.url).pathname;
// 单文件上限:超过就先截断,免得长期跑把 .tmp 撑爆(每次启动检查一次即可)
const LOG_MAX_BYTES = 2 * 1024 * 1024;
try {
  if (statSync(LOG_PATH).size > LOG_MAX_BYTES) writeFileSync(LOG_PATH, '');
} catch {}
function log(msg) {
  try { appendFileSync(LOG_PATH, `${new Date().toISOString()} pid=${process.pid} ${msg}\n`); } catch {}
}
process.on('exit', (code) => log(`exit code=${code}`));
process.on('SIGTERM', () => log('SIGTERM'));
process.on('SIGINT', () => log('SIGINT'));
process.on('uncaughtException', (e) => log(`uncaughtException ${e?.stack || e}`));
process.on('unhandledRejection', (e) => log(`unhandledRejection ${e?.stack || e}`));
// ──────────────────────────────────────────────────────────────────────

const URL = process.env.MDIO_MCP_URL || 'http://127.0.0.1:8011/mcp';
const MOUNTS = process.env.MDIO_VFS_MOUNTS || '';
const GATEWAY = process.env.MDIO_GATEWAY_BIN || 'mdio';
const READY_TIMEOUT_MS = 30000;

log(`start cwd=${process.cwd()} ppid=${process.ppid} URL=${URL} GATEWAY=${GATEWAY} node=${process.version}`);

async function probe() {
  try {
    const res = await fetch(URL, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', id: 'probe', method: 'tools/list' }),
      signal: AbortSignal.timeout(3000),
    });
    if (res.ok) { await res.arrayBuffer(); return true; }
    log(`probe http ${res.status}`);
  } catch (e) {
    log(`probe threw: ${e?.name} ${e?.message} ${e?.cause?.code || e?.cause?.message || ''}`);
  }
  return false;
}

async function ensureGateway() {
  const ok = await probe();
  log(`probe result: ${ok ? 'ok (gateway already up)' : 'FAIL -> will spawn gateway'}`);
  if (ok) return;
  const env = { ...process.env };
  if (MOUNTS) env.MDIO_VFS_MOUNTS = MOUNTS;
  let child;
  try {
    child = spawn(GATEWAY, ['serve'], { detached: true, stdio: 'ignore', env });
    log(`spawned gateway pid=${child.pid ?? 'none'}`);
    child.on('error', (e) => log(`gateway spawn error: ${e?.code} ${e?.message}`));
  } catch (e) {
    log(`gateway spawn threw: ${e?.code} ${e?.message}`);
  }
  child?.unref();
  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 500));
    if (await probe()) { log('gateway became ready'); return; }
  }
  log('gateway NOT ready after timeout — continuing to forward anyway');
}

await ensureGateway();
log('attaching stdin reader');

const rl = readline.createInterface({ input: process.stdin, terminal: false });
rl.on('close', () => log('stdin closed'));
rl.on('line', async (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { log(`stdin non-JSON line (${line.length} bytes)`); return; }
  log(`-> ${msg.method ?? 'response'} id=${msg.id ?? '-'}`);
  try {
    const res = await fetch(URL, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(msg),
    });
    if (res.status === 204) { log(`<- 204 (notification) id=${msg.id ?? '-'}`); return; } // 通知:无响应体
    const text = await res.text();
    log(`<- http ${res.status} ct=${res.headers.get('content-type')} bytes=${text.length} id=${msg.id ?? '-'}`);
    process.stdout.write(text + '\n');
  } catch (e) {
    log(`<- forward FAILED id=${msg.id ?? '-'}: ${e?.name} ${e?.message} ${e?.cause?.code || e?.cause?.message || ''}`);
    process.stdout.write(JSON.stringify({
      jsonrpc: '2.0',
      id: msg.id ?? null,
      error: { code: -32603, message: 'bridge error: ' + String(e) },
    }) + '\n');
  }
});
