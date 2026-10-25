import React from 'react';
import ReactDOM from 'react-dom/client';
import './styles.css';
import './themes.css';
import WorkspaceApp from './WorkspaceApp';
import './visual.css';
import './contrast.css';
import './layout-v024.css';
import './layout-v026.css';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode><WorkspaceApp /></React.StrictMode>,
);
