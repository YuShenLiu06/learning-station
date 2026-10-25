// Build-time copy of the already installed, pinned Node; no download/install/network.
import fs from 'node:fs';
import {createHash} from 'node:crypto';
const manifest=JSON.parse(fs.readFileSync(new URL('../runtime/manifest.json',import.meta.url)));
const spec=process.platform==='win32'?manifest.windowsX64:manifest;
const binary=fs.readFileSync(process.execPath);
if(process.version!==spec.version||process.platform!==spec.platform||process.arch!==spec.arch||createHash('sha256').update(binary).digest('hex')!==spec.sha256)throw Error('构建需使用runtime/manifest.json所记录的现有Node版本；未下载或替换运行时。');
const target=new URL(`../runtime/node${process.platform==='win32'?'.exe':''}`,import.meta.url);if(!fs.existsSync(target)||fs.realpathSync(target)!==fs.realpathSync(process.execPath))fs.copyFileSync(process.execPath,target);fs.chmodSync(target,0o755);
process.stdout.write(`随包Node ${process.version} ${process.arch} 已核对。\n`);
