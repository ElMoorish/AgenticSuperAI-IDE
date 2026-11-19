import { create } from "zustand";

interface D { isOpen: boolean; open: () => void; close: () => void }
export const useSettingsDialog = create<D>((set) => ({
  isOpen: false,
  open: () => set({ isOpen: true }),
  close: () => set({ isOpen: false }),
}));
