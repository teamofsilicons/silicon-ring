import {copyFile} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
const root = new URL('../', import.meta.url);
await copyFile(fileURLToPath(new URL('scripts/install.sh', root)), fileURLToPath(new URL('web/dist/install.sh', root)));
