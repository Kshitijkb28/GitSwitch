import { ReactNode } from "react";

interface CardProps {
  children: ReactNode;
  className?: string;
  active?: boolean;
  onClick?: () => void;
}

export function Card({
  children,
  className = "",
  active = false,
  onClick,
}: CardProps) {
  return (
    <div
      onClick={onClick}
      className={`rounded-xl border bg-zinc-800/50 p-4 transition-all duration-150 ${
        active
          ? "border-emerald-500/50 shadow-lg shadow-emerald-500/10"
          : "border-zinc-700/50 hover:border-zinc-600"
      } ${onClick ? "cursor-pointer" : ""} ${className}`}
    >
      {children}
    </div>
  );
}
