// 设置中心:左侧导航 + 右侧分区。
// 大类:基础设置(外观)/ 配置(可视化 config.toml)/ 项目(工作区)/ 会话 / 关于。

import { useState } from "react";
import { ArrowLeft, Palette, Wrench, Folder, MessageSquare, Info } from "lucide-react";
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

const TAB_DESCRIPTIONS: Record<SettingsTab, string> = {
  appearance: "外观、字体和显示密度。",
  config: "配置 OneMore agent、模型与执行权限。",
  projects: "管理本机保存的项目和工作区分组。",
  sessions: "按项目查看、重命名和清理历史会话。",
  about: "版本与应用信息。",
};

interface SettingsModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export default function SettingsModal({ open, onOpenChange }: SettingsModalProps) {
  const [tab, setTab] = useState<SettingsTab>("appearance");

  if (!open) return null;

  return (
    <div className="settings-shell">
      <div className="settings-window">
        <nav className="settings-nav">
          <button type="button" className="settings-back-button" onClick={() => onOpenChange(false)}>
            <ArrowLeft size={15} />
            <span>返回应用</span>
          </button>
          <div className="settings-nav-title">设置</div>
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
            <div>
              <h2>{NAV_ITEMS.find((i) => i.id === tab)?.label}</h2>
              <p>{TAB_DESCRIPTIONS[tab]}</p>
            </div>
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
