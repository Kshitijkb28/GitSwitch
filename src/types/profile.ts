export interface Profile {
  id: string;
  name: string;
  git_name: string;
  git_email: string;
  ssh_key_path: string | null;
  is_default: boolean;
  allow_push: boolean;
  signing_enabled: boolean;
  directories: string[];
  /** `owner/`, `owner/repo` or a full URL glob: clones of these commit as this profile wherever they are. */
  remote_patterns: string[];
  /** The GitHub account this profile pushes as, when known. */
  github_login: string | null;
  /** HTTPS clones in its folders push over SSH with its key; off unless turned on (null). */
  push_over_ssh?: boolean | null;
  created_at: string;
  updated_at: string;
}
