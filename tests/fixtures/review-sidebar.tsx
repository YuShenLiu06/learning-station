// Development-only actual App harness. No user data, real model or native writes.
import React from 'react';
import { createRoot } from 'react-dom/client';
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import '../../src/style.css';
import '../../src/courseSidebar.css';
if (!import.meta.env.DEV) throw new Error('仅开发环境可用');
Object.assign(globalThis, {isTauri:true});
mockWindows('review-sidebar-fixture');
const courseId='review_fixture_course', id='review_fixture_note';
const doc={id,path:`courses/${courseId}/materials/documents/${id}.md`,metadata:{id,kind:'section',title:'审批入口验收笔记',parentId:courseId,goals:[],tags:[],links:[]},body:'# 课程笔记\n\n这是已保存的原文。',version:'v1'};
const session={id:'review_fixture_chat',courseId,sectionId:id,title:'隔离验收会话',version:'s1',entries:[],contextManifest:[]};
const drafts=new Map<string,unknown>([[`section:${id}`,{key:`section:${id}`,data:{body:'# 我的未保存草稿\n\n定位审批后，这段文字必须仍在编辑器里。'},version:'draft',updatedAt:'fixture'}]]);
let proposals: {id:string;courseId:string;path:string;original:string;content:string;status:string}[]=[];
let polls=0;
mockIPC(async(command,args)=>{
 switch(command){
  case 'get_app_state':return {root:'/isolated-review-fixture',activeDocumentId:id,documents:[{...doc.metadata,path:doc.path,version:doc.version}],model:{configured:false,provider:'custom',model:'',baseUrl:'',usageGate:''}};
  case 'get_library_tree':return {nodes:[{id:courseId,kind:'course',title:'文件审批隔离课程',path:'',version:'v1'},{...doc.metadata,courseId,path:doc.path,version:doc.version}],warnings:[]};
  case 'get_ui_preferences':return {fontSize:16,assistantWidth:346,sidebarWidth:280,sidebarCollapsed:false,expandedNodeIds:[courseId]};
  case 'save_ui_preferences':return args;
  case 'get_ui_draft':return drafts.get(args.key)||null;
  case 'save_ui_draft':{const value={key:args.key,data:args.data,version:'draft',updatedAt:'fixture'};drafts.set(args.key,value);return value;}
  case 'open_document':return structuredClone(doc);
  case 'start_session':return {session:structuredClone(session)};
  case 'get_session':return structuredClone(session);
  case 'load_session_context':return {text:'',manifest:[],warnings:[],truncated:false};
  case 'course_sessions':return {courseId,activeSessionId:session.id,sessions:[session]};
  case 'list_course_files':return [{id:'file-review-fixture',path:`documents/${id}.md`,name:`${id}.md`,documentId:id,directory:false}];
  case 'edit_proposals':
   // Allow entering the real editor first, then simulate the tool's pending receipt.
   if(++polls===8)proposals=[1,2].map(n=>({id:`fixture_proposal_${n}`,courseId,path:`documents/${id}.md`,original:doc.body,content:`# 课程笔记\n\n这是第 ${n} 份等待批准的建议。`,status:'pending'}));
   return proposals.map(p=>({id:p.id,stamp:'1'}));
  case 'edit_proposal':return proposals.find(p=>p.id===args.id);
  case 'decide_edit_proposal':if(args.accept)throw Error('本验收草稿未保存，不允许接受');proposals=proposals.filter(p=>p.id!==args.id);return {persisted:false,status:'rejected'};
  case 'list_todos':case 'list_notifications':return [];
  case 'record_ui_metric':return null;
  default:throw Error(`隔离 fixture 不支持 ${command}`);
 }
},{shouldMockEvents:true});
const {App}=await import('../../src/App');
createRoot(document.getElementById('root')!).render(<App/>);
