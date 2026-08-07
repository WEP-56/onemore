import { useEffect } from "react";
import { useStore } from "./store";
import TopBar from "../components/TopBar";
import ConnectPanel from "../components/ConnectPanel";
import ModePanel from "../components/ModePanel";
import Transcript from "../components/Transcript";
import Composer from "../components/Composer";
import RunMonitor from "../components/RunMonitor";
import ApprovalDialog from "../components/ApprovalDialog";
import Diagnostics from "../components/Diagnostics";

/// 首屏直接进入可操作界面：连接表单 → 工作台（模式列 + transcript + 底部状态条）。
export default function App() {
  const init = useStore((s) => s.init);
  const conn = useStore((s) => s.conn);
  const mode = useStore((s) => s.mode);

  useEffect(() => {
    void init();
  }, [init]);

  return (
    <div className="app">
      <TopBar />
      <div className="app-body">
        {conn === "disconnected" ? (
          <ConnectPanel />
        ) : (
          <>
            <ModePanel />
            <main className="main-area">
              {mode === "diagnostics" ? (
                <Diagnostics />
              ) : (
                <div className="workbench">
                  <Transcript />
                  <Composer />
                </div>
              )}
            </main>
          </>
        )}
      </div>
      {conn !== "disconnected" && <RunMonitor />}
      <ApprovalDialog />
    </div>
  );
}
