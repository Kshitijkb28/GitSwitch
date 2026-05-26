import { useState, useEffect, useCallback } from "react";
import type { Profile } from "../types/profile";
import * as api from "../lib/api";

export function useProfiles() {
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);
      const data = await api.getProfiles();
      setProfiles(data);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      if (msg.includes("Tauri runtime not available")) {
        setProfiles([]);
      } else {
        setError(msg);
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const defaultProfile = profiles.find((p) => p.is_default) ?? null;

  return { profiles, loading, error, refresh, defaultProfile };
}
