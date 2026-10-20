var hide = true;
var precision = 1;

var objects = [];
var velocity;
var path;
var timer;

var hitOptions = { stroke: true, tolerance: precision };

function onMouseDown(event) {
	velocity = 0;
	path = new Path([event.point, event.point]);
}

function onMouseUp(event) {
	velocity = (event.point - event.downPoint).normalize(velocity);
	objects.push({ path: path, point: event.point, velocity: velocity });
	if (!timer)
		timer = setInterval(nextStep, 1);
}

function onMouseDrag(event) {
	velocity = (velocity + event.delta.length) * 0.8;
	path.lastSegment.remove();
	path.add(event.point);
}

function nextStep() {
	for (var i = objects.length - 1; i >= 0; i--) {
		var obj = objects[i];
		var path = obj.path;
		if (!path.isInserted()) {
			objects.splice(i, 1);
			continue;
		}
		var velocity = obj.velocity;
		path.lastSegment.remove();
		var p1 = obj.point;
		var p2 = p1 + velocity;
		var length = velocity.length;
		var amount = Math.ceil(length / precision);
		if (amount > 0) {
			var vector = velocity.normalize(length / amount);
			// Hide path so it will be ignored in tests
			path.visible = !hide;
			for (var j = 0; j < amount; j++) {
				var result = project.hitTest(p1, hitOptions);
				if (result && result.location) {
					var curve = result.location.curve;
					var normal = curve.getNormalAtTime(result.location.time);
					var tangent = curve.getTangentAtTime(result.location.time);
					p2 = result.point;
					velocity = -velocity;
					var angle = velocity.getDirectedAngle(tangent);
					velocity = velocity.rotate(180 - 2 * angle);
					path.add(result.point);
					// Avoid another reflexion with the same object just
					// after being reflected:
					var next;
					do {
						p2 += velocity;
						next = project.hitTest(p2, {
							stroke: true, tolerance: precision * 2
						});
					} while (next && next.item == result.item);
					break;
				}
				p1 += vector;
			}
			path.visible = true;
		}
		path.add(p2);
		obj.point = p2;
		obj.velocity = velocity;
	}
}
