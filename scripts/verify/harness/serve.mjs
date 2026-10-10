// Serve the built app and stay up until killed — for a check that is not
// driven from this process (webkit.sh). Prints the address on its first line.
import { serve, DIST_OK } from "./browser.mjs";

if (!DIST_OK) {
  console.error("dist/ is missing — run `npm run build` first");
  process.exit(1);
}
const server = await serve();
console.log(server.url);
process.on("SIGTERM", () => { server.close(); process.exit(0); });
process.on("SIGINT", () => { server.close(); process.exit(0); });
setInterval(() => {}, 1 << 30);
