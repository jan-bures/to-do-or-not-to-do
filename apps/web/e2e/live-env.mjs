// Where the live test server listens and keeps its data (shared by live-server.mjs and the tests).
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const PORT = Number(process.env.LIVE_PORT ?? 47412);
export const DATA = join(dirname(fileURLToPath(import.meta.url)), ".live-data");
