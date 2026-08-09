import { cn } from "@/lib/utils";

const appIcon = new URL("../../src-tauri/icons/128x128.png", import.meta.url).href;

export default function BrandMark({ className }: { className?: string }) {
  return (
    <span className={cn("brand-mark", className)} aria-hidden="true">
      <img src={appIcon} alt="" draggable={false} />
    </span>
  );
}
