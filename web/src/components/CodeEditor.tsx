// Sözdizimi renklendirmeli metin düzenleyici (CodeMirror).
// Büyük bir kütüphane olduğundan yalnızca bir dosya açıldığında yüklenir.

import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { php } from "@codemirror/lang-php";
import { sql } from "@codemirror/lang-sql";
import { xml } from "@codemirror/lang-xml";
import { yaml } from "@codemirror/lang-yaml";
import type { Extension } from "@codemirror/state";
import { EditorState } from "@codemirror/state";
import { oneDark } from "@codemirror/theme-one-dark";
import { EditorView, keymap } from "@codemirror/view";
import { basicSetup } from "codemirror";
import { useEffect, useRef } from "react";

/** Dosya uzantısına göre dil desteği; bilinmeyen türler düz metin olarak açılır. */
function languageFor(filename: string): Extension[] {
  const extension = filename.toLowerCase().split(".").pop() ?? "";
  switch (extension) {
    case "php":
    case "phtml":
      return [php()];
    case "html":
    case "htm":
    case "twig":
    case "vue":
      return [html()];
    case "css":
    case "scss":
    case "less":
      return [css()];
    case "js":
    case "mjs":
    case "cjs":
    case "jsx":
      return [javascript({ jsx: true })];
    case "ts":
    case "tsx":
      return [javascript({ jsx: true, typescript: true })];
    case "json":
      return [json()];
    case "md":
    case "markdown":
      return [markdown()];
    case "sql":
      return [sql()];
    case "yml":
    case "yaml":
      return [yaml()];
    case "xml":
    case "svg":
      return [xml()];
    default:
      return [];
  }
}

/** Panelin renk belirteçlerine uyan açık tema. */
const lightTheme = EditorView.theme({
  "&": { backgroundColor: "var(--surface)", color: "var(--ink)" },
  ".cm-gutters": { backgroundColor: "var(--sunken)", color: "var(--ink-3)", border: "none" },
  ".cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--accent-soft) 45%, transparent)" },
  ".cm-activeLineGutter": { backgroundColor: "var(--accent-soft)" },
});

const baseTheme = EditorView.theme({
  "&": { height: "100%", fontSize: "13px" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.55" },
  "&.cm-focused": { outline: "none" },
});

interface CodeEditorProps {
  filename: string;
  initialValue: string;
  dark: boolean;
  onChange: (value: string) => void;
  /** Ctrl/Cmd+S ile kaydetme. */
  onSave: () => void;
}

export default function CodeEditor({ filename, initialValue, dark, onChange, onSave }: CodeEditorProps) {
  const host = useRef<HTMLDivElement>(null);
  // Düzenleyici bir kez kurulur; geri çağrıların güncel hâli referanstan okunur.
  const handlers = useRef({ onChange, onSave });
  handlers.current = { onChange, onSave };

  useEffect(() => {
    if (!host.current) return;
    const view = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: initialValue,
        extensions: [
          basicSetup,
          keymap.of([
            {
              key: "Mod-s",
              preventDefault: true,
              run: () => {
                handlers.current.onSave();
                return true;
              },
            },
          ]),
          ...languageFor(filename),
          baseTheme,
          dark ? oneDark : lightTheme,
          EditorView.updateListener.of((update) => {
            if (update.docChanged) handlers.current.onChange(update.state.doc.toString());
          }),
        ],
      }),
    });
    view.focus();
    return () => view.destroy();
    // Başlangıç değeri yalnızca kurulumda kullanılır; sonraki değişiklikler düzenleyicinin kendi durumundadır.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filename, dark]);

  return <div ref={host} className="h-full overflow-hidden" />;
}
