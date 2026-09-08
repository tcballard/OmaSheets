pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id:view
    property var chart:null
    property color textColor:"#ffffff"
    property color backgroundColor:"#101216"
    property var colors:["#70a5ff","#77cc88","#c58cff"]
    onChartChanged:plot.requestPaint()
    onTextColorChanged:plot.requestPaint()
    onColorsChanged:plot.requestPaint()

    Canvas {
        id:plot
        Layout.fillWidth:true
        Layout.preferredHeight:230
        onWidthChanged:requestPaint()
        onHeightChanged:requestPaint()
        onPaint: {
            const ctx=getContext("2d");ctx.reset();
            if (!view.chart || !view.chart.series.length) return;
            const chart=view.chart, count=chart.categories.length;
            ctx.font="11px sans-serif";ctx.fillStyle=view.textColor;
            if (chart.kind==="pie") {
                const positive=chart.series[0].values.map(value=>value===null ? 0 : Math.max(0,value));
                const peak=Math.max(1,...positive);
                const values=positive.map(value=>value/peak);
                const total=values.reduce((a,b)=>a+b,0); if(total<=0) {ctx.fillText("No positive values to plot",16,30);return;}
                let angle=-Math.PI/2;const radius=Math.min(width*0.30,height*0.42),cx=width*0.33,cy=height/2;
                values.forEach((value,index)=>{const end=angle+value/total*Math.PI*2;
                    ctx.fillStyle=view.colors[index%view.colors.length];ctx.beginPath();ctx.moveTo(cx,cy);ctx.arc(cx,cy,radius,angle,end);ctx.closePath();ctx.fill();angle=end;
                    if(index<10){ctx.fillRect(width*0.66,16+index*19,9,9);ctx.fillStyle=view.textColor;ctx.fillText(chart.categories[index].slice(0,25),width*0.66+15,24+index*19);}
                }); return;
            }
            const numbers=chart.series.flatMap(series=>series.values.filter(value=>value!==null));
            if(!numbers.length)return;
            const scale=Math.max(1,...numbers.map(value=>Math.abs(value)));
            const scaled=numbers.map(value=>value/scale);
            let minimum=Math.min(0,...scaled),maximum=Math.max(0,...scaled);if(minimum===maximum)maximum=minimum+1;
            const left=64,top=12,bottom=height-36,right=width-12,w=right-left,h=bottom-top;
            function y(value){return bottom-(value-minimum)/(maximum-minimum)*h;}
            ctx.strokeStyle=view.textColor;ctx.globalAlpha=0.25;ctx.lineWidth=1;
            for(let tick=0;tick<=4;tick++){const value=minimum+(maximum-minimum)*tick/4;ctx.beginPath();ctx.moveTo(left,y(value));ctx.lineTo(right,y(value));ctx.stroke();}
            ctx.globalAlpha=1;ctx.fillStyle=view.textColor;
            for(let tick=0;tick<=4;tick++){const value=minimum+(maximum-minimum)*tick/4;ctx.fillText(Number((value*scale).toPrecision(4)).toString(),2,y(value)+4);}
            const slot=w/Math.max(1,count),step=Math.max(1,Math.ceil(count/10));
            for(let index=0;index<count;index+=step)ctx.fillText(chart.categories[index].slice(0,12),left+index*slot,bottom+22);
            chart.series.forEach((series,seriesIndex)=>{
                ctx.strokeStyle=view.colors[seriesIndex%view.colors.length];ctx.fillStyle=ctx.strokeStyle;ctx.lineWidth=2;
                let drawing=false;ctx.beginPath();
                series.values.forEach((rawValue,index)=>{
                    if(rawValue===null){drawing=false;return;}
                    const value=rawValue/scale;
                    const x=left+(index+0.5)*slot;
                    if(chart.kind==="line"){if(drawing)ctx.lineTo(x,y(value));else ctx.moveTo(x,y(value));drawing=true;}
                    else {const bar=slot*0.8/chart.series.length;ctx.fillRect(left+index*slot+slot*0.1+seriesIndex*bar,Math.min(y(value),y(0)),Math.max(1,bar-1),Math.max(1,Math.abs(y(value)-y(0))));}
                });ctx.stroke();
            });
        }
    }
    Label {
        Layout.fillWidth:true
        text:view.chart ? (view.chart.kind==="pie" ? "Pie uses positive values from " + view.chart.series[0].name : view.chart.series.map(series=>series.name).join(" · ")) : "Create or choose a chart."
        textFormat:Text.PlainText
        wrapMode:Text.WordWrap
        color:view.textColor
    }
    ScrollView {
        id:table
        Layout.fillWidth:true
        Layout.fillHeight:true
        clip:true
        contentWidth:Math.max(availableWidth,view.chart ? (view.chart.series.length+1)*110 : 0)
        Column {
            width:table.contentWidth
            Row {
                Repeater {
                    model:view.chart ? ["Category"].concat(view.chart.series.map(series=>series.name)) : []
                    delegate:Label {required property string modelData;width:table.contentWidth/((view.chart ? view.chart.series.length : 0)+1);height:26;text:modelData;font.bold:true;textFormat:Text.PlainText;elide:Text.ElideRight;color:view.textColor}
                }
            }
            Repeater {
                model:view.chart ? view.chart.categories.length : 0
                delegate:Row {
                    id:dataRow
                    required property int index
                    Repeater {
                        model:view.chart ? [view.chart.categories[dataRow.index]].concat(view.chart.series.map(series=>series.values[dataRow.index])) : []
                        delegate:Label {required property var modelData;width:table.contentWidth/((view.chart ? view.chart.series.length : 0)+1);height:25;text:modelData===null ? "—" : String(modelData);textFormat:Text.PlainText;elide:Text.ElideRight;color:view.textColor}
                    }
                }
            }
        }
    }
}
