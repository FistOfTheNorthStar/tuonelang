// nbody — the equivalent-semantics Go peer for the tuonelang nbody workload
// (the Benchmarks Game's n-body): the same five bodies in parallel columns,
// the same pairwise update order, 1,000,000 steps of dt = 0.01. Every product
// that feeds an addition is wrapped in an explicit float64 conversion: the Go
// spec lets the compiler fuse x*y + z into one FMA (and on arm64 it does),
// while an explicit conversion forces the product to round first, matching
// the unfused C, Rust, and tuonelang peers. Exit byte:
// int64(-energy * 1e9) % 256.
package main

import (
	"math"
	"os"
)

const bodies = 5
const steps = 1000000
const pi = 3.141592653589793
const daysPerYear = 365.24

var x, y, z, vx, vy, vz, mass [bodies]float64

func system() {
	p := [bodies][7]float64{
		{0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0},
		{4.84143144246472090e+00, -1.16032004402742839e+00, -1.03622044471123109e-01,
			1.66007664274403694e-03, 7.69901118419740425e-03, -6.90460016972063023e-05,
			9.54791938424326609e-04},
		{8.34336671824457987e+00, 4.12479856412430479e+00, -4.03523417114321381e-01,
			-2.76742510726862411e-03, 4.99852801234917238e-03, 2.30417297573763929e-05,
			2.85885980666130812e-04},
		{1.28943695621391310e+01, -1.51111514016986312e+01, -2.23307578892655734e-01,
			2.96460137564761618e-03, 2.37847173959480950e-03, -2.96589568540237556e-05,
			4.36624404335156298e-05},
		{1.53796971148509165e+01, -2.59193146099879641e+01, 1.79258772950371181e-01,
			2.68067772490389322e-03, 1.62824170038242295e-03, -9.51592254519715870e-05,
			5.15138902046611451e-05},
	}
	solarMass := float64(4.0*pi) * pi
	for i := 0; i < bodies; i++ {
		x[i], y[i], z[i] = p[i][0], p[i][1], p[i][2]
		vx[i] = p[i][3] * daysPerYear
		vy[i] = p[i][4] * daysPerYear
		vz[i] = p[i][5] * daysPerYear
		mass[i] = p[i][6] * solarMass
	}
}

func offsetMomentum() {
	px, py, pz := 0.0, 0.0, 0.0
	for i := 0; i < bodies; i++ {
		px = px + float64(vx[i]*mass[i])
		py = py + float64(vy[i]*mass[i])
		pz = pz + float64(vz[i]*mass[i])
	}
	solarMass := float64(4.0*pi) * pi
	vx[0] = -px / solarMass
	vy[0] = -py / solarMass
	vz[0] = -pz / solarMass
}

func dist2(dx, dy, dz float64) float64 {
	return float64(float64(dx*dx)+float64(dy*dy)) + float64(dz*dz)
}

func energy() float64 {
	e := 0.0
	for i := 0; i < bodies; i++ {
		e = e + float64(float64(0.5*mass[i])*dist2(vx[i], vy[i], vz[i]))
		for j := i + 1; j < bodies; j++ {
			dx, dy, dz := x[i]-x[j], y[i]-y[j], z[i]-z[j]
			e = e - float64(mass[i]*mass[j])/math.Sqrt(dist2(dx, dy, dz))
		}
	}
	return e
}

func advance(dt float64) {
	for i := 0; i < bodies; i++ {
		for j := i + 1; j < bodies; j++ {
			dx, dy, dz := x[i]-x[j], y[i]-y[j], z[i]-z[j]
			d2 := dist2(dx, dy, dz)
			mag := dt / float64(d2*math.Sqrt(d2))
			mi, mj := mass[i], mass[j]
			vx[i] = vx[i] - float64(float64(dx*mj)*mag)
			vy[i] = vy[i] - float64(float64(dy*mj)*mag)
			vz[i] = vz[i] - float64(float64(dz*mj)*mag)
			vx[j] = vx[j] + float64(float64(dx*mi)*mag)
			vy[j] = vy[j] + float64(float64(dy*mi)*mag)
			vz[j] = vz[j] + float64(float64(dz*mi)*mag)
		}
	}
	for i := 0; i < bodies; i++ {
		x[i] = x[i] + float64(dt*vx[i])
		y[i] = y[i] + float64(dt*vy[i])
		z[i] = z[i] + float64(dt*vz[i])
	}
}

func main() {
	system()
	offsetMomentum()
	for step := 0; step < steps; step++ {
		advance(0.01)
	}
	os.Exit(int(int64(-energy()*1e9) % 256))
}
