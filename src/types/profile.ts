export interface Profile {
  id: string;
  name: string;
  git_name: string;
  git_email: string;
  ssh_key_path: string | null;
  github_token: string | null;
  is_default: boolean;
  directories: string[];
  created_at: string;
  updated_at: string;
}
