interface BadgeProps {
  children: React.ReactNode;
  variant?: "default" | "success" | "warning" | "error";
}

export function Badge({ children, variant = "default" }: BadgeProps) {
  const variants = {
    default: "bg-zinc-700 text-zinc-300",
    success: "bg-emerald-500/20 text-emerald-400 border border-emerald-500/30",
    warning: "bg-amber-500/20 text-amber-400 border border-amber-500/30",
    error: "bg-red-500/20 text-red-400 border border-red-500/30",
  };

  return (
    <span
      className={`inline-flex items-center px-2 py-0.5 text-xs font-medium rounded-md ${variants[variant]}`}
    >
      {children}
    </span>
  );
}
