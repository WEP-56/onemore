import { useEffect, type CSSProperties } from "react";
import { useStore } from "./store";
import Sidebar from "@/components/Sidebar";
import ChatArea from "@/components/ChatArea";
import RightPanel from "@/components/RightPanel";
import SettingsModal from "@/components/SettingsModal";
import ApprovalDialog from "@/components/ApprovalDialog";
import ErrorToasts from "@/components/ErrorToasts";
import MainTopbar from "@/components/MainTopbar";
import { useResizablePanels } from "@/hooks/useResizablePanels";
import { useSidebarToggles } from "@/hooks/useSidebarToggles";

export default function App() {
  const init = useStore((s) => s.init);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const setSettingsOpen = useStore((s) => s.setSettingsOpen);
  const { sidebarWidth, rightPanelWidth, onSidebarResizeStart, onRightPanelResizeStart } =
    useResizablePanels();
  const { sidebarCollapsed, toggleSidebar } = useSidebarToggles();

  useEffect(() => {
    void init();
  }, [init]);

  return (
    <div
      className={`app layout-desktop${sidebarCollapsed ? " sidebar-collapsed" : ""}`}
      style={
        {
          "--sidebar-width": `${sidebarWidth}px`,
          "--right-panel-width": `${rightPanelWidth}px`,
        } as CSSProperties
      }
    >
      <Sidebar />
      <div
        className="sidebar-resizer"
        role="separator"
        aria-orientation="vertical"
        onMouseDown={onSidebarResizeStart}
      />
      <section className="main">
        <MainTopbar
          sidebarCollapsed={sidebarCollapsed}
          onToggleSidebar={toggleSidebar}
          onOpenSettings={() => setSettingsOpen(true)}
        />
        <div className="content">
          <div className="content-layer content-layer--chat">
            <ChatArea />
          </div>
        </div>
        <div
          className="right-panel-resizer"
          role="separator"
          aria-orientation="vertical"
          onMouseDown={onRightPanelResizeStart}
        />
        <div className="right-panel">
          <RightPanel />
        </div>
      </section>
      <ApprovalDialog />
      <ErrorToasts />
      <SettingsModal open={settingsOpen} onOpenChange={setSettingsOpen} />
    </div>
  );
}
