import { create } from 'zustand';

interface DocsState {
  helpOpen: boolean;
  paletteOpen: boolean;
  selectedEntryId: string | null;
  openHelp: () => void;
  closeHelp: () => void;
  openPalette: () => void;
  closePalette: () => void;
  selectEntry: (id: string | null) => void;
}

export const useDocsStore = create<DocsState>((set) => ({
  helpOpen: false,
  paletteOpen: false,
  selectedEntryId: null,
  openHelp: () => set({ helpOpen: true, paletteOpen: false }),
  closeHelp: () => set({ helpOpen: false, selectedEntryId: null }),
  openPalette: () => set({ paletteOpen: true, helpOpen: false }),
  closePalette: () => set({ paletteOpen: false, selectedEntryId: null }),
  selectEntry: (id) => set({ selectedEntryId: id }),
}));
