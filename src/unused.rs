#[derive(Debug, Deserialize, Serialize)]
struct ApiResult<'a> {
    version: f32,
    generator: &'a str,
    elements: Vec<Element>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum Element {
    Node(Node),
    Way(Way),
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
struct Node {
    id: usize,
    lat: f32,
    lon: f32,
}

impl PartialEq for Node {
    fn eq(&self, other: &Node) -> bool {
        self.id == other.id
    }
}
impl Eq for Node {}

impl Hash for Node {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Node {
    fn project<P: SphereProject>(self, projection: &P) -> (f32, f32) {
        projection.project(self.lat, self.lon)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Way {
    id: usize,
    nodes: Vec<usize>,
}

impl Way {
    fn iter<'way, 'nodes>(
        &'way self,
        nodes: &'nodes HashMap<usize, Node>,
    ) -> WayIter<'nodes, 'way> {
        WayIter {
            nodes: nodes,
            way: &self.nodes,
        }
    }

    fn extend(&mut self, other: Way) {
        if other.nodes.len() >= 2 {
            self.nodes.extend_from_slice(&other.nodes[1..])
        }
    }

    fn extend_reverse(&mut self, other: Way) {
        if other.nodes.len() >= 2 {
            self.nodes.extend(other.nodes.iter().rev())
        }
    }
}

struct WayIter<'nodes, 'way> {
    nodes: &'nodes HashMap<usize, Node>,
    way: &'way [usize],
}

impl<'node_vec, 'way> Iterator for WayIter<'node_vec, 'way> {
    type Item = Node;

    fn next(&mut self) -> Option<Node> {
        let (next_node, remaining_way) = self.way.split_first()?;
        self.way = remaining_way;

        let coordinates = *self.nodes.get(next_node)?;

        Some(coordinates)
    }
}

fn combine_ways(mut ways: Vec<Way>) -> Vec<Way> {
    let mut result = vec![];
    loop {
        let mut way = match ways.pop() {
            Some(w) => w,
            None => break,
        };
        'b: loop {
            let last_node = way.nodes.last().unwrap_or(&0);
            for (index, other) in ways.iter().enumerate() {
                if other.nodes.first().unwrap_or(&0) == last_node {
                    way.extend(ways.remove(index));
                    continue 'b;
                }
                if other.nodes.last().unwrap_or(&0) == last_node {
                    way.extend_reverse(ways.remove(index));
                    continue 'b;
                }
            }
            // no more appendable paths found
            result.push(way);
            break 'b;
        }
    }
    result
}

trait SphereProject {
    fn project(&self, lat: f32, lon: f32) -> (f32, f32);
}

struct LambertCylindricalProjection {
    central_meridian: f32,
}

impl SphereProject for LambertCylindricalProjection {
    fn project(&self, lat: f32, lon: f32) -> (f32, f32) {
        (
            lon.to_radians(),
            -(lat - self.central_meridian).to_radians().sin(),
        )
    }
}

struct OrthographicProjection {
    center: (f32, f32),
}

impl SphereProject for OrthographicProjection {
    fn project(&self, lat: f32, lon: f32) -> (f32, f32) {
        let (lat_0, lon_0) = (self.center.0.to_radians(), self.center.1.to_radians());
        let (lat, lon) = (lat.to_radians() - lat_0, lon.to_radians() - lon_0);
        let x = lat.cos() * lon.sin();
        let y = lat.sin();
        (x, -y)
    }
}

fn create_svg() -> Result<(), Box<dyn Error>> {
    let file = fs::read("resources/result1.json")?;

    let result: ApiResult = serde_json::from_slice(&file)?;
    let nodes: HashMap<usize, Node> = result
        .elements
        .iter()
        .filter_map(|element| match element {
            Element::Node(node) => Some((node.id, *node)),
            _ => None,
        })
        .collect();

    let ways: Vec<Way> = result
        .elements
        .into_iter()
        .filter_map(|element| match element {
            Element::Way(way) => Some(way),
            _ => None,
        })
        .collect();

    println!("number of ways: {}", ways.len());
    let ways = combine_ways(ways);
    println!("number of ways (combined): {}", ways.len());

    let projection = OrthographicProjection {
        center: (45.0, 10.0),
    };
    let projected_ways = ways
        .into_iter()
        .map(|way| {
            way.iter(&nodes)
                .map(|node| node.project(&projection))
                .collect::<LineString<_>>()
        })
        .collect::<MultiLineString<_>>();

    let projected_ways = projected_ways.simplifyvw(&1e-8);
    let bounding_box = projected_ways.bounding_rect().unwrap();

    let mut document = Document::new();
    let mut data = Data::new();
    for way in projected_ways {
        let mut iter = way.points_iter();
        if let Some(point) = iter.next() {
            data = data.move_to(point.x_y())
        } else {
            continue;
        }
        for point in iter {
            data = data.line_to(point.x_y())
        }
        data = data.close();
    }
    let path = Path::new()
        .set("fill", "red")
        .set("stroke", "black")
        .set("stroke-width", 0.0001)
        .set("d", data);
    document = document.add(path);
    document = document.set(
        "viewBox",
        (
            bounding_box.min.x,
            bounding_box.min.y,
            bounding_box.width(),
            bounding_box.height(),
        ),
    );
    document = document.set("height", "800px");

    svg::save("image.svg", &document)?;

    Ok(())
}