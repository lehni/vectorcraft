////////////////////////////////////////////////////////////////////////////////
// Values

var values = {
	scale: 0.9
};

////////////////////////////////////////////////////////////////////////////////
// Mouse handling

var path;
function onMouseDown(event) {
	path = new Path();
	path.add(event.point);
}

function onMouseDrag(event) {
	path.add(event.point);
}

function onMouseUp(event) {
	path.simplify();
	var group = new Group([path]);
	for(var i = 0; i < 100; i++) {
		var lastCurve = path.lastCurve;
		var point2 = lastCurve.getPointAtTime(1);
		var angle2 = lastCurve.getTangentAtTime(1).angle;
		var clone = path.clone();
		group.addChild(clone);
		clone.scale(values.scale);
		var firstB = clone.curves[0];
		var point1 = firstB.getPointAtTime(0);
		var angle1 = firstB.getTangentAtTime(0).angle;
		clone.rotate(angle2 - angle1, point1);
		clone.position += point2 - point1;
		path = clone;
		if (clone.length < 1) {
			clone.remove();
			break;
		}
	}
}

////////////////////////////////////////////////////////////////////////////////
// Interface

var components = {
	scale: {
		label: 'Scale',
		type: 'slider',
		range: [0.5, 1]
	}
};

var palette = new Palette('Grow', components, values);
