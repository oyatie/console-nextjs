import type { Metadata } from "next";
import "@/styles/globals.css";
import "@/styles/browser-session.css";

export const dynamic = "force-dynamic";

export const metadata: Metadata = {
  title: "오야티 | Oyatie",
  description: "업무와 개인 기록",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="ko">
      <body>
        <script id="console-browser-restoration-guard" dangerouslySetInnerHTML={{ __html: `(()=>{let blocked=false,reloaded=false;const hide=()=>{blocked=true;document.documentElement.setAttribute('data-console-browser-blocked','1');const region=document.getElementById('console-browser-private-region');if(region){region.hidden=true;region.style.setProperty('display','none','important')}const recovery=document.getElementById('console-browser-recovery');if(recovery)recovery.hidden=false};Object.defineProperty(window,'__consoleBrowserRestorationGuard',{value:Object.freeze({get blocked(){return blocked}})});addEventListener('pagehide',hide);addEventListener('pageshow',event=>{if(event.persisted){hide();if(!reloaded){reloaded=true;location.reload()}}})})()` }} />
        {children}
      </body>
    </html>
  );
}
