// 设置中心:左侧导航 + 右侧分区。
// 大类:基础设置(外观)/ 配置(可视化 config.toml)/ 项目(工作区)/ 会话 / 关于。

import { useState } from "react";
import { X, Palette, Wrench, Folder, MessageSquare, Info } from "lucide-react";
import { cn } from "@/lib/utils";
import AppearanceSection from "@/components/settings/AppearanceSection";
import ConfigSection from "@/components/settings/ConfigSection";
import ProjectsSection from "@/components/settings/ProjectsSection";
import SessionsSection from "@/components/settings/SessionsSection";
import AboutSection from "@/components/settings/AboutSection";

type SettingsTab = "appearance" | "config" | "projects" | "sessions" | "about";

const NAV_ITEMS: { id: SettingsTab; label: string; icon: React.ReactNode }[] = [
  { id: "appearance", label: "基础设置", icon: <Palette /> },
  { id: "config", label: "配置", icon: <Wrench /> },
  { id: "projects", label: "项目", icon: <Folder /> },
  { id: "sessions", label: "会话", icon: <MessageSquare /> },
  { id: "about", label: "关于", icon: <Info /> },
];

interface SettingsModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export default function SettingsModal({ open, onOpenChange }: SettingsModalProps) {
  const [tab, setTab] = useState<SettingsTab>("appearance");

  if (!open) return null;

  return (
    <div className="settings-shell" onClick={() => onOpenChange(false)}>
      <div className="settings-window" onClick={(e) => e.stopPropagation()}>
        <nav className="settings-nav">
          <div className="settings-nav-title">OnemoreGui</div>
          {NAV_ITEMS.map((item) => (
            <button
              key={item.id}
              type="button"
              className={cn("settings-nav-item", tab === item.id && "is-active")}
              onClick={() => setTab(item.id)}
            >
              {item.icon}
              {item.label}
            </button>
          ))}
        </nav>
        <div className="settings-content">
          <div className="settings-content-header">
            <h2>{NAV_ITEMS.find((i) => i.id === tab)?.label}</h2>
            <button
              type="button"
              className="flex h-7 w-7 items-center justify-center rounded-md text-[var(--text-muted)] transition-colors hover:bg-[var(--surface-hover)] hover:text-[var(--text-strong)]"
              title="关闭设置"
              onClick={() => onOpenChange(false)}
            >
              <X size={15} />
            </button>
          </div>
          <div className="settings-body">
            {tab === "appearance" && <AppearanceSection />}
            {tab === "config" && <ConfigSection />}
            {tab === "projects" && <ProjectsSection />}
            {tab === "sessions" && <SessionsSection />}
            {tab === "about" && <AboutSection />}
          </div>
        </div>
      </div>
    </div>
  );
}
