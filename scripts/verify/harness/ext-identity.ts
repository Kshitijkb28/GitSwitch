// The VS Code extension's own resolver (vscode-extension/src/resolve.ts),
// run against a real repository and the profiles under $HOME — what the
// extension would show for it: {profile, mismatch, email} as JSON.
import { resolveIdentity, loadProfiles, profilesPath } from "../../../vscode-extension/src/resolve";
import { git } from "../../../vscode-extension/src/gitrun";

const repo = process.argv[2];
try {
  const root = await git(repo, ["rev-parse", "--show-toplevel"]);
  const r = await resolveIdentity(root, (cwd, args) => git(cwd, args), loadProfiles(profilesPath(process.env.HOME)));
  console.log(JSON.stringify({ profile: r.profile?.name ?? null, mismatch: r.mismatch, email: r.email }));
} catch (e) {
  console.log(JSON.stringify({ error: String(e) }));
}
