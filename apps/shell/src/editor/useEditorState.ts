import { useSyncExternalStore } from "react";
import { models, type EditorState } from "./models";

export const useEditorState = (): EditorState => useSyncExternalStore(models.subscribe, models.getState);
