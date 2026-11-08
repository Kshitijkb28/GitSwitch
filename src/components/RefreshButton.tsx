import { RefreshCw } from "lucide-react";
import { Button } from "./Button";

/**
 * The same Refresh on every page: re-reads what the page shows. The label
 * never changes while it runs (the icon spins), so a person — or a harness —
 * always finds it under the same name.
 */
export function RefreshButton({
  onClick,
  busy = false,
  disabled = false,
  title = "Re-read this page",
}: {
  onClick: () => void;
  busy?: boolean;
  disabled?: boolean;
  title?: string;
}) {
  return (
    <Button variant="secondary" size="sm" onClick={onClick} disabled={busy || disabled} title={title}>
      <RefreshCw size={14} className={busy ? "animate-spin" : ""} />
      Refresh
    </Button>
  );
}
