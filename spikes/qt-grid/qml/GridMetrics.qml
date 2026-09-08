import QtQuick

QtObject {
    id:metrics
    property int rowCount:1
    property int columnCount:1
    property real defaultRowHeight:27
    property real defaultColumnWidth:132
    property var view:({})
    readonly property var rows:axis(rowCount,defaultRowHeight,view.row_heights || [],view.hidden_rows || [])
    readonly property var columns:axis(columnCount,defaultColumnWidth,view.column_widths || [],[])
    readonly property int frozenRows:Math.min(rowCount,view.frozen_rows || 0)
    readonly property int frozenColumns:Math.min(columnCount,view.frozen_columns || 0)
    readonly property real frozenHeight:position(rows,frozenRows)
    readonly property real frozenWidth:position(columns,frozenColumns)
    readonly property real contentHeight:position(rows,rowCount)
    readonly property real contentWidth:position(columns,columnCount)

    function axis(count,base,overrides,hidden) {
        const sizes=Object.create(null);
        for(const entry of overrides) sizes[entry[0]]=entry[1];
        for(const index of hidden) sizes[index]=0;
        const indices=Object.keys(sizes).map(Number).sort((a,b)=>a-b),delta=[0];
        for(const index of indices) delta.push(delta[delta.length-1]+sizes[index]-base);
        return {count:count,base:base,sizes:sizes,indices:indices,delta:delta};
    }
    function position(axis,index) {
        index=Math.max(0,Math.min(axis.count,index));
        let low=0,high=axis.indices.length;
        while(low<high){const mid=Math.floor((low+high)/2);if(axis.indices[mid]<index)low=mid+1;else high=mid;}
        return index*axis.base+axis.delta[low];
    }
    function size(axis,index) {return axis.sizes[index]===undefined ? axis.base : axis.sizes[index];}
    function indexAt(axis,pixel) {
        let low=0,high=axis.count;
        while(low<high){const mid=Math.floor((low+high)/2);if(position(axis,mid+1)<=pixel)low=mid+1;else high=mid;}
        return Math.min(axis.count-1,low);
    }
    function rowPosition(row){return position(rows,row);}
    function columnPosition(column){return position(columns,column);}
    function rowHeight(row){return size(rows,row);}
    function columnWidth(column){return size(columns,column);}
    function screenRow(row,offset){return rowPosition(row)-(row<frozenRows ? 0 : offset);}
    function screenColumn(column,offset){return columnPosition(column)-(column<frozenColumns ? 0 : offset);}
    function visible(axis,offset,extent,frozen,limit) {
        const result=[];
        for(let index=0;index<frozen && position(axis,index)<extent && result.length<limit;index++)if(size(axis,index)>0)result.push(index);
        const boundary=position(axis,frozen);
        if(boundary>=extent)return result;
        let index=Math.max(frozen,indexAt(axis,offset+boundary));
        while(index<axis.count && position(axis,index)<offset+extent+axis.base && result.length<limit){
            if(size(axis,index)>0)result.push(index);
            index=Math.max(index+1,indexAt(axis,position(axis,index+1)));
        }
        return result;
    }
    function visibleRows(offset,height){return visible(rows,offset,height,frozenRows,128);}
    function visibleColumns(offset,width){return visible(columns,offset,width,frozenColumns,80);}
    function mergeAt(row,column) {
        for(const range of view.merges || [])if(row>=range.row && row<range.row+range.rows && column>=range.column && column<range.column+range.columns)return range;
        return null;
    }
    function cells(visibleRows,visibleColumns,x,y,width,height) {
        const result=[],seen=Object.create(null);
        function add(row,column){const key=row+":"+column;if(!seen[key]){seen[key]=true;result.push({row:row,column:column});}}
        for(const row of visibleRows)for(const column of visibleColumns)add(row,column);
        // A visible part of a merge needs its anchor even when that anchor has scrolled away.
        for(const range of view.merges || []){
            const left=screenColumn(range.column,x),top=screenRow(range.row,y);
            const right=left+columnPosition(range.column+range.columns)-columnPosition(range.column);
            const bottom=top+rowPosition(range.row+range.rows)-rowPosition(range.row);
            if(right>0 && bottom>0 && left<width && top<height)add(range.row,range.column);
        }
        return result;
    }
}
