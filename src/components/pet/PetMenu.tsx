import { useEffect } from "react";
import { api } from "../../api/tauri";
import "./PetMenu.css";

// ペットを右クリックしたときのメニュー(ペットの行動の設計書の9節)。外を押すと本体(pet/mod.rs)が閉じる。
function PetMenu() {
  useEffect(() => {
    document.body.style.background = "transparent";
  }, []);

  return (
    <div className="pet-menu" onContextMenu={(e) => e.preventDefault()}>
      <button type="button" className="pet-menu__item" onClick={() => api.petPickStart().catch(() => undefined)}>
        ここに移動して
      </button>
    </div>
  );
}

export default PetMenu;
