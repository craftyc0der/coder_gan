use unit_converter::temperature;
use unit_converter::distance;

fn main() {
    println!("=== Unit Converter Demo ===\n");

    println!("--- Temperature ---");
    println!("  100°C  → {:.2}°F", temperature::celsius_to_fahrenheit(100.0));
    println!("   32°F  → {:.2}°C", temperature::fahrenheit_to_celsius(32.0));
    println!("    0°C  → {:.2} K", temperature::celsius_to_kelvin(0.0));
    println!("373.15 K → {:.2}°C", temperature::kelvin_to_celsius(373.15));

    println!("\n--- Distance ---");
    println!("  1 mile  → {:.5} km", distance::miles_to_km(1.0));
    println!("  1 km    → {:.5} mi", distance::km_to_miles(1.0));
    println!("  1 meter → {:.5} ft", distance::meters_to_feet(1.0));
    println!("  1 foot  → {:.5} m",  distance::feet_to_meters(1.0));

    println!("\nAll conversions complete.");
}
