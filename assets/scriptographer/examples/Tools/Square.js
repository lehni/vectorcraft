////////////////////////////////////////////////////////////////////////////////
// Values

var values = {
	tolerance: 5
};

////////////////////////////////////////////////////////////////////////////////
// Mouse handling

var prevSeg, curSeg, path;
function onMouseDown(event) {
	path = new Path([event.point, event.point]);
	prevSeg = path.firstSegment;
	curSeg = path.lastSegment;
}

function onMouseUp(event) {
	// Scriptographer called path.pointsToCurves(0, 0, 1, 0.001) here. Paper.js's
	// simplify() would round the corners, so only drop the zero-length curves:
	path.reduce();
}

function onMouseDrag(event) {
	if (Key.isDown('space')) {
		path.add(event.point);
		curSeg = path.lastSegment;
	} else {
		var diff = (event.point - prevSeg.point).abs();
		var minDiff = Math.min(diff.x, diff.y);
		if (diff.x < diff.y) {
			curSeg.point = new Point(prevSeg.point.x, event.point.y);
		} else {
			curSeg.point = new Point(event.point.x, prevSeg.point.y);
		}
		if (minDiff > values.tolerance) {
			prevSeg = curSeg;
			path.add(curSeg);
			curSeg = path.lastSegment;
		}
	}
}

////////////////////////////////////////////////////////////////////////////////
// Interface

var components = {
	tolerance: {
		label: 'Tolerance',
		min: 0,
		steppers: true
	}
};

var palette = new Palette('Square', components, values);
