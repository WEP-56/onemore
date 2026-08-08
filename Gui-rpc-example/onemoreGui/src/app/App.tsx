import { useEffect } from "react";
import { useStore } from "./store";
import Sidebar from "@/components/Sidebar";
import ChatArea from "@/components/ChatArea";
import RightPanel from "@/components/RightPanel";
import SettingsModal from "@/components/SettingsModal";
import ApprovalDialog from "@/components/ApprovalDialog";

export default function App() {
  const init = useStore((s) => s.init);

  useEffect(() => {
    void init();
  }, [init]);

  return (
    <div className="flex h-full overflow-hidden">
      <Sidebar />
      <ChatArea />
      <RightPanel />
      <ApprovalDialog />
      <SettingsModal />
    </div>
  );
}
