import { memo, useMemo, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import clsx from "clsx";
import { linkifyCitations, parseCiteHref, type Citation } from "../../lib/citations";
import { openExternal } from "../../lib/platform";
import { toast } from "../../store/toasts";

export interface MarkdownProps {
  content: string;
  className?: string;
  /** Called when a citation chip is clicked. Without it citations render as plain text. */
  onCite?: (c: Citation) => void;
  /** Custom chip content (e.g. recording title for library citations). */
  renderCite?: (c: Citation, label: ReactNode) => ReactNode;
}

function MarkdownImpl({ content, className, onCite, renderCite }: MarkdownProps) {
  const source = useMemo(() => (onCite ? linkifyCitations(content) : content), [content, onCite]);

  const components = useMemo<Components>(
    () => ({
      p: ({ children }) => <p dir="auto">{children}</p>,
      li: ({ children }) => <li dir="auto">{children}</li>,
      h1: ({ children }) => <h1 dir="auto">{children}</h1>,
      h2: ({ children }) => <h2 dir="auto">{children}</h2>,
      h3: ({ children }) => <h3 dir="auto">{children}</h3>,
      blockquote: ({ children }) => <blockquote dir="auto">{children}</blockquote>,
      td: ({ children }) => <td dir="auto">{children}</td>,
      img: () => null,
      a: ({ href, children }) => {
        const cite = parseCiteHref(href);
        if (cite && onCite) {
          return (
            <button
              type="button"
              dir="ltr"
              onClick={() => onCite(cite)}
              className="mx-0.5 inline-flex h-[18px] -translate-y-px items-center rounded-md bg-accent-soft px-1.5 align-middle text-[11px] font-semibold text-accent tabular-nums no-underline transition hover:brightness-95 dark:hover:brightness-125"
              title="Play this part"
            >
              {renderCite ? renderCite(cite, children) : children}
            </button>
          );
        }
        return (
          <a
            href={href}
            onClick={(e) => {
              e.preventDefault();
              if (href && /^https?:\/\//i.test(href)) {
                openExternal(href).catch((err) => toast.error("Couldn't open link", err));
              }
            }}
          >
            {children}
          </a>
        );
      },
    }),
    [onCite, renderCite],
  );

  return (
    <div className={clsx("prose-app", className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {source}
      </ReactMarkdown>
    </div>
  );
}

export const Markdown = memo(MarkdownImpl);
