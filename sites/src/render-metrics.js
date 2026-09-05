// Explicit opt-in diagnostics. No telemetry, network collection, or persistence.
export function instrumentRenderer(engine,renderer,kind){
  if(!new URLSearchParams(location.search).has('perf'))return;
  const gl=renderer.getContext();
  const extension=gl.getExtension('EXT_disjoint_timer_query_webgl2');
  const record={kind,cpu:[],gpu:[],intervals:[],calls:[],triangles:[],gpuTimer:!!extension,frames:0};
  window.__glasshousePerf??=[];window.__glasshousePerf.push(record);
  let previous=0,pending=null;
  const original=engine.draw.bind(engine);
  engine.draw=(...args)=>{
    if(pending&&gl.getQueryParameter(pending,gl.QUERY_RESULT_AVAILABLE)){
      if(!gl.getParameter(extension.GPU_DISJOINT_EXT))record.gpu.push(gl.getQueryParameter(pending,gl.QUERY_RESULT)/1e6);
      gl.deleteQuery(pending);pending=null;
    }
    const query=extension&&!pending?gl.createQuery():null;
    if(query)gl.beginQuery(extension.TIME_ELAPSED_EXT,query);
    const start=performance.now();original(...args);const end=performance.now();
    if(query){gl.endQuery(extension.TIME_ELAPSED_EXT);pending=query;}
    if(record.frames++>30){
      record.cpu.push(end-start);
      if(previous&&start-previous<250)record.intervals.push(start-previous);
      record.calls.push(renderer.info.render.calls);record.triangles.push(renderer.info.render.triangles);
    }
    previous=start;
    for(const values of [record.cpu,record.gpu,record.intervals,record.calls,record.triangles])if(values.length>600)values.shift();
  };
}
