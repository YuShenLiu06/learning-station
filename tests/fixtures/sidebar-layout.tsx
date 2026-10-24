// Isolated browser acceptance harness; no native service or user files.
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { mockIPC } from '@tauri-apps/api/mocks';
import type { LibraryNode } from '../../src/types';
import '../../src/style.css';
import '../../src/courseSidebar.css';
Object.assign(globalThis, { isTauri: true });
const { WorkspaceSidebar } = await import('../../src/WorkspaceSidebar');
const course: LibraryNode = {id:'fixture-course',kind:'course',title:'课程侧栏验收',path:'',version:'v1'};
const note: LibraryNode = {id:'fixture-note',kind:'section',parentId:course.id,courseId:course.id,title:'课堂笔记',path:'courses/fixture-course/materials/documents/note.md',version:'v1'};
const newNote: LibraryNode = {...note,id:'fixture-new',title:'外部新增笔记',path:'courses/fixture-course/materials/documents/new.md'};
let external=false;
mockIPC(async(command) => {
 if(command==='list_course_files')return [{id:'file-note',path:'documents/note.md',name:'note.md',documentId:note.id,directory:false},...(external?[{id:'file-new',path:'documents/new.md',name:'new.md',documentId:newNote.id,directory:false}]:[])];
 if(command==='reveal_course_file')return null;
 throw Error(`Unexpected fixture command: ${command}`);
});
function Fixture(){
 const[nodes,setNodes]=useState([course,note]);const[active,setActive]=useState(note.id);const[collapsed,setCollapsed]=useState(false);const[expanded,setExpanded]=useState<string[]>([]);const[width,setWidth]=useState(480);const[message,setMessage]=useState('点击文件以打开');const noop=()=>{};
 return <div className={`app-shell ${collapsed?'sidebar-collapsed':''}`} style={{'--sidebar-width':`${width}px`,'--assistant-width':'346px'} as React.CSSProperties}>
 <header className="workspace-topbar"><button onClick={()=>setCollapsed(v=>!v)}>展开／收起侧栏</button><button onClick={()=>{external=true;setMessage('已模拟外部新增文件，请点刷新课程文件');}}>模拟外部新增文件</button><span>保存宽度：{width}px</span></header>
 <WorkspaceSidebar nodes={nodes} activeId={active} expandedIds={expanded} onExpanded={setExpanded} collapsed={collapsed} onToggleCollapsed={()=>setCollapsed(v=>!v)} sidebarWidth={width} onSidebarWidth={setWidth} onRefreshNodes={async()=>{setNodes([course,note,newNote]);}} onOpen={id=>{setActive(id);setMessage(`已打开 ${id}`);}} onNew={kind=>setMessage(`已选择新建 ${kind}`)} onImport={()=>setMessage('已选择导入')} onLearning={noop} onExplorations={noop} onHistory={noop} onReview={noop} onTasks={noop} onTrash={noop} onTrashNode={noop} onInject={noop} injectedIds={[]} onRemoveInjection={noop} canInject={false} onSettings={noop} view="lesson" loading={false} dirtyIds={[]} taskCount={0} modelReady={false} blocked={false}/>
 <main className="main-area" style={{padding:24}}><h1>实际侧栏组件验收</h1><p role="status">{message}</p><p>查找 note.md 应显示“课堂笔记”。</p><textarea aria-label="保留输入" defaultValue="缩放与折叠时保留的输入"/><p>本页使用实际样式和组件，文件列表为合成 IPC。</p></main>
 <aside className="assistant-panel" style={{padding:20}}><h2>学习助手</h2><button style={{alignSelf:'flex-end'}}>右侧控件完整可见</button><p>窗口变窄时，课程侧栏显示宽度夹紧，保存的 480px 保持不变。</p></aside></div>;
}
createRoot(document.getElementById('root')!).render(<Fixture/>);
