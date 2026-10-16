import test from 'node:test';import assert from 'node:assert/strict';
import { visibleLibraryRows, ancestorIds, containingCourse } from '../src/treePolicy.ts';
import { assetReferenceSegments, citationForAsset, safeAssetImage } from '../src/assetReferences.ts';
const nodes=[{id:'course',kind:'course',title:'Course',parentId:null,order:0},{id:'folder',kind:'folder',title:'Chapter',parentId:'course',order:0},{id:'lesson',kind:'section',title:'English passage',parentId:'folder',order:0},{id:'explore',kind:'exploration',title:'My question',parentId:'folder',order:1},{id:'exercise',kind:'exercise',title:'Practice',parentId:'lesson',order:0},{id:'legacy',kind:'knowledge',title:'Old root',parentId:null,order:1}];
test('persistent library hierarchy keeps stable nodes, ordered descendants, collapsed branches and searchable ancestors',()=>{
 assert.deepEqual(visibleLibraryRows(nodes,[]).map(x=>x.node.id),['course','legacy']);
 assert.deepEqual(visibleLibraryRows(nodes,['course','folder']).map(x=>[x.node.id,x.depth]),[['course',0],['folder',1],['lesson',2],['explore',2],['legacy',0]]);
 assert.deepEqual(visibleLibraryRows(nodes,[],'English').map(x=>x.node.id),['course','folder','lesson']);
 assert.deepEqual(ancestorIds(nodes,'exercise'),['lesson','folder','course']);assert.equal(containingCourse(nodes,'explore').id,'course');
 const moved=nodes.map(x=>x.id==='lesson'?{...x,parentId:'course'}:x);assert.deepEqual(ancestorIds(moved,'exercise'),['lesson','course']);assert.equal(moved.find(x=>x.id==='exercise').id,'exercise');
});
test('malformed external hierarchy stays bounded and accessible without mutating legacy files',()=>{
 const cyclic=[{id:'a',kind:'folder',parentId:'b',title:'A'},{id:'b',kind:'folder',parentId:'a',title:'B'},{id:'orphan',kind:'section',parentId:'missing',title:'Orphan'}];const before=structuredClone(cyclic);
 const result=visibleLibraryRows(cyclic,['a','b']);assert.equal(result.length,3);assert.equal(new Set(result.map(x=>x.node.id)).size,3);assert.deepEqual(cyclic,before);
});
test('asset references never activate arbitrary URLs, paths or references inside code',()=>{
 const value=assetReferenceSegments('Text [code](asset:asset_safe)\n![image](asset:picture_1)\n![remote](https://example.test/x)\n[bad](asset:../../secret)\n`[literal](asset:ignore)`\n```py\n[another](asset:ignore_too)\n```');
 assert.deepEqual(value.filter(x=>x.kind==='asset').map(x=>[x.id,x.image]),[['asset_safe',false],['picture_1',true]]);
 assert.equal(assetReferenceSegments('```py\n[unfinished](asset:ignore)').filter(x=>x.kind==='asset').length,0);
 assert.equal(safeAssetImage('https://example.test/image.png'),undefined);assert.equal(safeAssetImage('data:image/svg+xml;base64,PHN2Zz4='),undefined);assert.equal(safeAssetImage('data:image/png;base64,aGVsbG8='),'data:image/png;base64,aGVsbG8=');
 assert.equal(citationForAsset({id:'safe',fileName:'code[1].py',mime:'text/plain'}),'[code_1_.py](asset:safe)');
});
