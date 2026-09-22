import {
  verifyFiles,
  verifyUpdaterReplaced,
  cleanupTestDir,
  getTestDir,
  getFileHash,
  waitForServer,
  runInstaller,
  assertExitOk,
  printLogFileIfExists,
  FLAGS,
} from './utils.mjs';
import express from 'express';
import path from 'path';
import fs from 'fs-extra';
import 'zx/globals';
import { $, usePwsh } from 'zx';
usePwsh();

// 夹具配置里 local-v2 写死 http://localhost:8080/test-app-v2.exe，这里必须用同一个端口
const PORT = 8080;
const FIXTURES_DIR = path.resolve('./fixtures');
const PACKAGE_PATH = '/test-app-v2.exe';

/**
 * 一个「愿意传一半就断」的服务器：复现下载中断。
 *
 * `truncate` 打开时，所有对更新包的 GET 都在发出 32 KiB 之后 destroy 掉 socket
 * （声明 1 MiB 的 Content-Length 却只给这么点）。保持打开状态直到调用方 `resume()`，
 * 这样安装器内部若重试也仍然失败，测试不会因为一次重试就变成「其实装成功了」。
 */
function startFlakyServer() {
  const app = express();
  const requests = [];
  let truncate = true;

  app.use((req, res, next) => {
    requests.push({ method: req.method, url: req.url });
    next();
  });

  app.use((req, res, next) => {
    if (!truncate || req.method !== 'GET' || req.url.split('?')[0] !== PACKAGE_PATH) {
      return next();
    }
    res.writeHead(200, {
      'Content-Type': 'application/octet-stream',
      'Content-Length': String(1024 * 1024),
    });
    res.write(Buffer.alloc(32 * 1024));
    setTimeout(() => res.socket?.destroy(), 30);
  });

  app.use(
    express.static(FIXTURES_DIR, {
      acceptRanges: true,
      lastModified: true,
      etag: true,
    }),
  );

  return new Promise((resolve) => {
    const server = app.listen(PORT, () =>
      resolve({
        server,
        requests,
        resume: () => {
          truncate = false;
        },
      }),
    );
  });
}

async function test() {
  const testDir = getTestDir('interrupted-download');
  const installerV1 = './fixtures/test-app-v1.exe';

  console.log(chalk.blue('=== Interrupted Download Test ==='));
  console.log(`Test directory: ${testDir}`);

  const v1UpdaterHash = await getFileHash('./fixtures/test-app-v1/updater.exe');
  const v2UpdaterHash = await getFileHash('./fixtures/test-app-v2/updater.exe');

  const { server, requests, resume } = await startFlakyServer();

  try {
    await waitForServer(`http://localhost:${PORT}${PACKAGE_PATH}`);

    console.log('Installing v1...');
    const installResult = await runInstaller(
      installerV1,
      [FLAGS, '-D', testDir],
      'v1 installation',
    );
    assertExitOk(installResult, 'v1 installation');

    const updaterPath = path.join(testDir, 'updater.exe');
    if ((await getFileHash(updaterPath)) !== v1UpdaterHash) {
      throw new Error('v1 updater hash mismatch before the interrupted update');
    }

    // === 第一次更新：包下载在开始处就被截断 ===
    console.log('Updating to v2 with a server that truncates the package...');
    const failedUpdate = await runInstaller(
      updaterPath,
      [FLAGS, '-D', testDir, '--source', 'local-v2'],
      'interrupted update',
    );
    console.log(
      chalk.gray(`  installer exit code: ${failedUpdate.exitCode}（不据此判定，只看落盘状态）`),
    );

    // 中断发生在任何文件写入之前：旧版本必须原封不动，更新器更不能消失
    // （自更新把正在运行的 exe 改名成 .instbak 之后，失败路径必须把它还原）。
    const oldVersion = await verifyFiles(testDir, [
      { path: 'app.exe', contains: 'APP_V1' },
      { path: 'config.json', contains: '"version": "1.0.0"' },
      { path: 'updater.exe', hash: v1UpdaterHash },
    ]);

    const leftovers = (await fs.readdir(testDir)).filter((entry) =>
      /\.(instbak|patching|patchold|old)$/i.test(entry),
    );

    if (oldVersion.failed.length > 0 || leftovers.length > 0) {
      console.error(chalk.red('✗ 中断后旧版本没有保持完整:'));
      oldVersion.failed.forEach((msg) =>
        console.error(chalk.red(`  - ${msg}`)),
      );
      leftovers.forEach((msg) =>
        console.error(chalk.red(`  - 残留临时文件: ${msg}`)),
      );
      await printLogFileIfExists();
      throw new Error('中断后旧版本没有保持完整');
    }
    console.log(
      chalk.green('✓ 中断后旧版本与更新器都还在，没有留下 .instbak / .patching 残留'),
    );

    // === 第二次更新：服务器恢复正常，必须能装完 ===
    console.log('Retrying the update with a healthy server...');
    requests.length = 0;
    resume();
    const retry = await runInstaller(
      updaterPath,
      [FLAGS, '-D', testDir, '--source', 'local-v2'],
      'retried update',
    );
    assertExitOk(retry, 'retried update');

    const verification = await verifyFiles(testDir, [
      { path: 'app.exe', contains: 'APP_V2' },
      { path: 'config.json', contains: '"version": "2.0.0"' },
      { path: 'feature.dll', size: 30720 },
      { path: 'data/assets.dat', size: 15360 },
      { path: 'data/new-assets.dat', size: 5120 },
      { path: 'updater.exe', hash: v2UpdaterHash },
    ]);
    const updaterCheck = await verifyUpdaterReplaced(testDir, v2UpdaterHash);

    // 更新包只按包 URL 取，从不逐个文件去请求同名资源：这是本仓库下载路径的形状
    // （整包 + Range），不是上游 DFS2 的批量会话。
    const perFileRequests = requests.filter((r) => {
      const url = r.url.split('?')[0];
      return (
        url !== PACKAGE_PATH &&
        /\.(exe|dll|dat|json|txt)$/i.test(url) &&
        url !== '/kachina.config.json'
      );
    });
    console.log(
      chalk.gray(
        `  更新期间对包的请求 ${requests.length} 次，逐个文件请求 ${perFileRequests.length} 次`,
      ),
    );

    const extraFailed = [];
    if (perFileRequests.length > 0) {
      extraFailed.push(
        `出现了逐个文件的下载请求（应为 0）: ${perFileRequests
          .map((r) => r.url)
          .join(', ')}`,
      );
    }

    if (
      verification.failed.length > 0 ||
      !updaterCheck.success ||
      extraFailed.length > 0
    ) {
      console.error(chalk.red('✗ 重跑验证失败:'));
      verification.failed.forEach((msg) =>
        console.error(chalk.red(`  - ${msg}`)),
      );
      if (!updaterCheck.success) {
        console.error(chalk.red(`  - ${updaterCheck.message}`));
      }
      extraFailed.forEach((msg) => console.error(chalk.red(`  - ${msg}`)));
      await printLogFileIfExists();
      throw new Error('重跑验证失败');
    }
    console.log(chalk.green('✓ 重跑后更新成功，更新器已换到 v2'));
  } catch (error) {
    console.error(chalk.red('Test failed:'), error.message);
    await printLogFileIfExists();
    process.exitCode = 1;
  } finally {
    server?.close();
    await cleanupTestDir(testDir);
  }
  process.exit(process.exitCode ?? 0);
}

test();
