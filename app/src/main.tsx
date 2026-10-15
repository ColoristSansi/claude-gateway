import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

// 空壳：七个页面（spec §10）从 M2 起逐个落地。
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <main>
      <h1>Claude Gateway</h1>
      <p>骨架已就位；页面随 M2 落地。</p>
    </main>
  </StrictMode>,
);
